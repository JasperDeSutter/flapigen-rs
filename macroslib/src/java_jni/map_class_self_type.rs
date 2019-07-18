use log::debug;
use smol_str::SmolStr;
use std::fmt::Write;
use syn::spanned::Spanned;

use super::{INTERNAL_PTR_MARKER, JAVA_RUST_SELF_NAME};
use crate::{
    error::{invalid_src_id_span, Result},
    source_registry::SourceId,
    typemap::{
        ast::{parse_ty_with_given_span_checked, DisplayToTokens, TypeName},
        ty::{ForeignConversationIntermediate, ForeignConversationRule, ForeignTypeS, RustType},
        utils::{boxed_type, convert_to_heap_pointer, unpack_from_heap_pointer},
        RustTypeIdx, TypeConvCode, TypeMap, FROM_VAR_TEMPLATE, TO_VAR_TEMPLATE,
    },
    types::{ForeignerClassInfo, SelfTypeDesc},
    WRITE_TO_MEM_FAILED_MSG,
};

pub(in crate::java_jni) fn register_typemap_for_self_type(
    conv_map: &mut TypeMap,
    class: &ForeignerClassInfo,
    this_type: RustType,
    self_desc: &SelfTypeDesc,
) -> Result<()> {
    debug!(
        "register_typemap_for_self_type: add implements SwigForeignClass for {}",
        this_type
    );
    let constructor_ret_type = &self_desc.constructor_ret_type;

    conv_map.find_or_alloc_rust_type_with_suffix(
        &parse_type! { jobject },
        &this_type.normalized_name,
        SourceId::none(),
    );

    conv_map.find_or_alloc_rust_type(constructor_ret_type, class.src_id);

    let this_type_inner = boxed_type(conv_map, &this_type);

    let code = format!("& {}", this_type_inner);
    let gen_ty = parse_ty_with_given_span_checked(&code, this_type_inner.ty.span());
    let this_type_ref = conv_map.find_or_alloc_rust_type(&gen_ty, class.src_id);

    let code = format!("&mut {}", this_type_inner);
    let gen_ty = parse_ty_with_given_span_checked(&code, this_type_inner.ty.span());
    let this_type_mut_ref = conv_map.find_or_alloc_rust_type(&gen_ty, class.src_id);

    register_rust_ty_conversation_rules(conv_map, &this_type)?;
    let self_type = conv_map.find_or_alloc_rust_type(&self_desc.self_type, class.src_id);
    register_main_foreign_types(
        conv_map,
        class,
        this_type.to_idx(),
        self_type.to_idx(),
        this_type_ref.to_idx(),
        this_type_mut_ref.to_idx(),
    )?;

    Ok(())
}

fn register_rust_ty_conversation_rules(conv_map: &mut TypeMap, this_type: &RustType) -> Result<()> {
    let (this_type_for_method, _code_box_this) =
        convert_to_heap_pointer(conv_map, this_type, "this");

    let jlong_ti: RustType = conv_map.find_or_alloc_rust_type_no_src_id(&parse_type! { jlong });
    let this_type_for_method_ty = &this_type_for_method.ty;
    let code = format!("& {}", DisplayToTokens(&this_type_for_method_ty));
    let gen_ty = parse_ty_with_given_span_checked(&code, this_type_for_method_ty.span());
    let this_type_ref = conv_map.find_or_alloc_rust_type(&gen_ty, this_type_for_method.src_id);
    //handle foreigner_class as input arg
    conv_map.add_conversation_rule(
        jlong_ti.to_idx(),
        this_type_ref.to_idx(),
        TypeConvCode::new2(
            format!(
                r#"
        let {to_var}: &{this_type} = unsafe {{
            jlong_to_pointer::<{this_type}>({from_var}).as_mut().unwrap()
        }};
    "#,
                to_var = TO_VAR_TEMPLATE,
                from_var = FROM_VAR_TEMPLATE,
                this_type = this_type_for_method.normalized_name,
            ),
            invalid_src_id_span(),
        )
        .into(),
    );
    let code = format!("&mut {}", DisplayToTokens(&this_type_for_method_ty));
    let gen_ty = parse_ty_with_given_span_checked(&code, this_type_for_method_ty.span());
    let this_type_mut_ref = conv_map.find_or_alloc_rust_type(&gen_ty, this_type_for_method.src_id);
    //handle foreigner_class as input arg
    conv_map.add_conversation_rule(
        jlong_ti.to_idx(),
        this_type_mut_ref.to_idx(),
        TypeConvCode::new2(
            format!(
                r#"
        let {to_var}: &mut {this_type} = unsafe {{
            jlong_to_pointer::<{this_type}>({from_var}).as_mut().unwrap()
        }};
    "#,
                to_var = TO_VAR_TEMPLATE,
                from_var = FROM_VAR_TEMPLATE,
                this_type = this_type_for_method.normalized_name,
            ),
            invalid_src_id_span(),
        )
        .into(),
    );

    Ok(())
}

fn register_main_foreign_types(
    conv_map: &mut TypeMap,
    class: &ForeignerClassInfo,
    this_type: RustTypeIdx,
    self_type: RustTypeIdx,
    this_type_ref: RustTypeIdx,
    this_type_mut_ref: RustTypeIdx,
) -> Result<()> {
    debug!(
        "register_main_foreign_types: ftype for this_type {}",
        conv_map[this_type]
    );
    let jlong_ty = parse_ty_with_given_span_checked("jlong", conv_map[this_type].ty.span());
    let out_val_prefix = format!("{}OutVal", class.name);
    let jlong_out_val_rty =
        conv_map.find_or_alloc_rust_type_with_suffix(&jlong_ty, &out_val_prefix, class.src_id);
    {
        conv_map.add_conversation_rule(
            this_type,
            jlong_out_val_rty.to_idx(),
            TypeConvCode::new2(
                format!(
                    r#"
    let {to_var}: jlong = <{this_type}>::box_object({from_var});
"#,
                    this_type = conv_map[this_type],
                    to_var = TO_VAR_TEMPLATE,
                    from_var = FROM_VAR_TEMPLATE,
                ),
                invalid_src_id_span(),
            )
            .into(),
        );
        let name_prefix: SmolStr = format!("/*{}*/", out_val_prefix).into();
        conv_map.alloc_foreign_type(ForeignTypeS {
            name: TypeName::new(
                format!("{}long", name_prefix),
                (class.src_id, class.name.span()),
            ),
            provides_by_module: vec![],
            from_into_rust: None,
            into_from_rust: Some(ForeignConversationRule {
                rust_ty: jlong_out_val_rty.to_idx(),
                intermediate: None,
            }),
            name_prefix: Some(name_prefix),
        })?;
    }
    let in_val_prefix = format!("{}InVal", class.name);
    let jlong_in_val_rty =
        conv_map.find_or_alloc_rust_type_with_suffix(&jlong_ty, &in_val_prefix, class.src_id);
    {
        let this_type2 = conv_map[this_type].clone();
        let (this_type_for_method, _code_box_this) =
            convert_to_heap_pointer(conv_map, &this_type2, "this");

        if class.smart_ptr_copy_derived {
            let unpack_code = unpack_from_heap_pointer(&this_type2, TO_VAR_TEMPLATE, true);
            conv_map.add_conversation_rule(
                jlong_in_val_rty.to_idx(),
                this_type,
                TypeConvCode::new2(
                    format!(
                        r#"
        let {to_var}: *mut {ptr_this_type} = unsafe {{
            jlong_to_pointer::<{ptr_this_type}>({from_var}).as_mut().unwrap()
        }};
    {unpack_code}
        let tmp: {this_type} = {to_var};
        let {to_var}: {this_type} = tmp.clone();
        ::std::mem::forget(tmp);
    "#,
                        to_var = TO_VAR_TEMPLATE,
                        from_var = FROM_VAR_TEMPLATE,
                        ptr_this_type = this_type_for_method,
                        this_type = this_type2,
                        unpack_code = unpack_code,
                    ),
                    invalid_src_id_span(),
                )
                .into(),
            );
        } else if class.copy_derived {
            conv_map.add_conversation_rule(
                jlong_in_val_rty.to_idx(),
                this_type,
                TypeConvCode::new2(
                    format!(
                        r#"
        let {to_var}: &{this_type} = unsafe {{
            jlong_to_pointer::<{this_type}>({from_var}).as_mut().unwrap()
        }};
        let {to_var}: {this_type} = {to_var}.clone();
    "#,
                        to_var = TO_VAR_TEMPLATE,
                        from_var = FROM_VAR_TEMPLATE,
                        this_type = this_type_for_method,
                    ),
                    invalid_src_id_span(),
                )
                .into(),
            );
        } else {
            let unpack_code = unpack_from_heap_pointer(&this_type2, TO_VAR_TEMPLATE, true);
            conv_map.add_conversation_rule(
                jlong_in_val_rty.to_idx(),
                this_type,
                TypeConvCode::new2(
                    format!(
                        r#"
        let {to_var}: *mut {this_type} = unsafe {{
            jlong_to_pointer::<{this_type}>({from_var}).as_mut().unwrap()
        }};
    {unpack_code}
    "#,
                        to_var = TO_VAR_TEMPLATE,
                        from_var = FROM_VAR_TEMPLATE,
                        this_type = this_type_for_method,
                        unpack_code = unpack_code,
                    ),
                    invalid_src_id_span(),
                )
                .into(),
            );
        }

        let name_prefix: SmolStr = format!("/*{}*/", in_val_prefix).into();
        conv_map.alloc_foreign_type(ForeignTypeS {
            name: TypeName::new(
                format!("{}long", name_prefix),
                (class.src_id, class.name.span()),
            ),
            provides_by_module: vec![],
            into_from_rust: None,
            from_into_rust: Some(ForeignConversationRule {
                rust_ty: jlong_in_val_rty.to_idx(),
                intermediate: None,
            }),
            name_prefix: Some(name_prefix),
        })?;
    }

    let mut java_code_in_val_to_long = format!(
        r#"
        long {to_var} = {from_var}.{class_raw_ptr};
"#,
        to_var = TO_VAR_TEMPLATE,
        from_var = FROM_VAR_TEMPLATE,
        class_raw_ptr = JAVA_RUST_SELF_NAME,
    );
    if !class.copy_derived && !class.smart_ptr_copy_derived {
        writeln!(
            &mut java_code_in_val_to_long,
            "        {from_var}.{class_raw_ptr} = 0;",
            from_var = FROM_VAR_TEMPLATE,
            class_raw_ptr = JAVA_RUST_SELF_NAME,
        )
        .expect(WRITE_TO_MEM_FAILED_MSG);
    }

    let class_ftype = ForeignTypeS {
        name: TypeName::new(class.name.to_string(), (class.src_id, class.name.span())),
        provides_by_module: vec![],
        into_from_rust: Some(ForeignConversationRule {
            rust_ty: this_type,
            intermediate: Some(ForeignConversationIntermediate {
                input_to_output: false,
                intermediate_ty: jlong_out_val_rty.to_idx(),
                conv_code: TypeConvCode::new(
                    format!(
                        "        {class_name} {out} = new {class_name}({internal_ptr_marker}.RAW_PTR, {var});",
                        class_name = class.name,
                        var = FROM_VAR_TEMPLATE,
                        out = TO_VAR_TEMPLATE,
                        internal_ptr_marker = INTERNAL_PTR_MARKER,
                    ),
                    invalid_src_id_span(),
                ),
            }),
        }),
        from_into_rust: Some(ForeignConversationRule {
            rust_ty: this_type,
            intermediate: Some(ForeignConversationIntermediate {
                input_to_output: false,
                intermediate_ty: jlong_in_val_rty.to_idx(),
                conv_code: TypeConvCode::new(
                    java_code_in_val_to_long,
                    invalid_src_id_span(),
                ),
            }),
        }),
        name_prefix: None,
    };
    conv_map.alloc_foreign_type(class_ftype)?;

    let jlong_ty = conv_map.ty_to_rust_type(&parse_type! { jlong });
    debug!(
        "register_main_foreign_types: ftype for this_type_ref {}",
        conv_map[this_type_ref]
    );
    let class_ftype_ref_in = ForeignTypeS {
        name: TypeName::new(
            format!("/*ref*/{}", class.name),
            (class.src_id, class.name.span()),
        ),
        provides_by_module: vec![],
        from_into_rust: Some(ForeignConversationRule {
            rust_ty: this_type_ref,
            intermediate: Some(ForeignConversationIntermediate {
                input_to_output: false,
                intermediate_ty: jlong_ty.to_idx(),
                conv_code: TypeConvCode::new(
                    format!(
                        "        long {out} = {from}.{self_raw_ptr};",
                        from = FROM_VAR_TEMPLATE,
                        out = TO_VAR_TEMPLATE,
                        self_raw_ptr = JAVA_RUST_SELF_NAME,
                    ),
                    invalid_src_id_span(),
                ),
            }),
        }),
        into_from_rust: None,
        name_prefix: Some("/*ref*/".into()),
    };
    conv_map.alloc_foreign_type(class_ftype_ref_in)?;

    debug!(
        "register_main_foreign_types: ftype for this_type_mut_ref {}",
        conv_map[this_type_mut_ref]
    );
    let class_ftype_mut_ref_in = ForeignTypeS {
        name: TypeName::new(
            format!("/*mut ref*/{}", class.name),
            (class.src_id, class.name.span()),
        ),
        provides_by_module: vec![],
        from_into_rust: Some(ForeignConversationRule {
            rust_ty: this_type_mut_ref,
            intermediate: Some(ForeignConversationIntermediate {
                input_to_output: false,
                intermediate_ty: jlong_ty.to_idx(),
                conv_code: TypeConvCode::new(
                    format!(
                        "        long {out} = {from}.{self_raw_ptr};",
                        from = FROM_VAR_TEMPLATE,
                        out = TO_VAR_TEMPLATE,
                        self_raw_ptr = JAVA_RUST_SELF_NAME,
                    ),
                    invalid_src_id_span(),
                ),
            }),
        }),
        into_from_rust: None,
        name_prefix: Some("/*mut ref*/".into()),
    };
    conv_map.alloc_foreign_type(class_ftype_mut_ref_in)?;

    if self_type != this_type {
        debug!(
            "register_main_foreign_types: self_type {} != this_type {}",
            conv_map[self_type], conv_map[this_type]
        );
        let self_type = conv_map[self_type].clone();
        {
            let gen_ty = parse_ty_with_given_span_checked(
                &format!("&mut {}", self_type),
                self_type.ty.span(),
            );
            let self_type_mut_ref = conv_map.find_or_alloc_rust_type(&gen_ty, class.src_id);

            debug!(
                "register_main_foreign_types: ftype for self_type_mut_ref {}",
                self_type_mut_ref
            );
            let class_ftype_mut_ref_in = ForeignTypeS {
                name: TypeName::new(
                    format!("/*ref 2*/{}", class.name),
                    (class.src_id, class.name.span()),
                ),
                provides_by_module: vec![],
                from_into_rust: Some(ForeignConversationRule {
                    rust_ty: self_type_mut_ref.to_idx(),
                    intermediate: Some(ForeignConversationIntermediate {
                        input_to_output: false,
                        intermediate_ty: jlong_ty.to_idx(),
                        conv_code: TypeConvCode::new(
                            format!(
                                "        long {out} = {from}.{self_raw_ptr};",
                                from = FROM_VAR_TEMPLATE,
                                out = TO_VAR_TEMPLATE,
                                self_raw_ptr = JAVA_RUST_SELF_NAME,
                            ),
                            invalid_src_id_span(),
                        ),
                    }),
                }),
                into_from_rust: None,
                name_prefix: Some("/*ref 2*/".into()),
            };
            conv_map.alloc_foreign_type(class_ftype_mut_ref_in)?;
        }
        {
            let code = format!("& {}", self_type);
            let gen_ty = parse_ty_with_given_span_checked(&code, self_type.ty.span());
            let self_type_ref = conv_map.find_or_alloc_rust_type(&gen_ty, class.src_id);

            let class_ftype_ref_in = ForeignTypeS {
                name: TypeName::new(
                    format!("/*mut ref 2*/{}", class.name),
                    (class.src_id, class.name.span()),
                ),
                provides_by_module: vec![],
                from_into_rust: Some(ForeignConversationRule {
                    rust_ty: self_type_ref.to_idx(),
                    intermediate: Some(ForeignConversationIntermediate {
                        input_to_output: false,
                        intermediate_ty: jlong_ty.to_idx(),
                        conv_code: TypeConvCode::new(
                            format!(
                                "        long {out} = {from}.{self_raw_ptr};",
                                from = FROM_VAR_TEMPLATE,
                                out = TO_VAR_TEMPLATE,
                                self_raw_ptr = JAVA_RUST_SELF_NAME,
                            ),
                            invalid_src_id_span(),
                        ),
                    }),
                }),
                into_from_rust: None,
                name_prefix: Some("/*mut ref 2*/".into()),
            };
            conv_map.alloc_foreign_type(class_ftype_ref_in)?;
        }
    }

    Ok(())
}
