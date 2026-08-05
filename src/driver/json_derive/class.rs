//! `to_json`/`from_json` derivation for a single `@json` class (see [`super`]). Emits Dream source
//! for the `extend <Class>` converter block; re-parsed through the normal pipeline by
//! [`super::generate_json_derives`].

use super::*;

/// If `ty` is `Option<T>`, returns `T`'s spelling (as written in source); otherwise `None`.
fn option_inner(ty: &dream_syntax::nodes::Type) -> Option<String> {
    match ty {
        dream_syntax::nodes::Type::Struct(token, Some(args))
            if token.text == "Option" && args.len() == 1 =>
        {
            Some(args[0].get_type())
        }
        _ => None,
    }
}

/// Generates `extend <Class> { fun to_json(): JsonValue {...} static fun from_json(v): <Class> {...} }`
/// source for a single `@json` class, or `None` (after reporting a diagnostic) if a field type is
/// outside the supported set (primitives, `string`, other `@json` classes, and arrays of those).
pub(super) fn generate_json_extend(
    struct_decl: &dream_syntax::nodes::struct_node::StructDeclarationNode,
    json_names: &HashSet<String>,
    jsonable: &HashSet<String>,
    diagnostics: &mut DiagnosticBag,
) -> Option<String> {
    let name = &struct_decl.name.text;
    // Generic parameter names (`Box<T>` -> ["T"]). A field typed by one of these is serialized
    // through the object protocol (`x.to_json()` / `T.from_json(...)`), resolved per concrete
    // instantiation by the monomorphizer. The `extend` and `from_json` are emitted with the same
    // parameter list so the derive attaches to the generic template.
    let generic_params: Vec<String> = struct_decl
        .generic_parameters
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default();
    let is_type_param = |t: &str| generic_params.iter().any(|p| p == t);
    let mut to_body = String::from("        let __o = JsonValue.dict();\n");
    let mut from_prelude = String::new();
    // `from_json` reconstructs the value by calling the class's field-order constructor positionally,
    // so a `@json` class must declare a `constructor` taking its fields in declaration order.
    let mut from_fields: Vec<String> = Vec::new();

    for field in &struct_decl.fields {
        let fname = &field.name.text;
        let ftype = field.type_token.text.as_str();

        if has_json_ignore(&field.attributes) {
            match json_ignore_default(ftype, &field.field_type) {
                Some(default) => {
                    from_fields.push(default);
                }
                None => {
                    diagnostics.report_error(
                        format!(
                            "@json class '{}' field '{}' has @json_ignore but type '{}' has no zero default (use `Option<{}>` or remove @json_ignore)",
                            name, fname, ftype, ftype
                        ),
                        Some(field.name.position),
                    );
                    return None;
                }
            }
            continue;
        }

        let mut json_key = fname.to_string();
        if let Some(prop_attr) = field
            .attributes
            .iter()
            .find(|a| a.name.text == PROPERTY_NAME_ATTR)
        {
            if let Some(arg) = prop_attr.args.first() {
                json_key = arg.text.trim_matches('"').to_string();
            }
        }

        // Optional field (`Option<T>`): a JSON `null` maps to/from `None`, otherwise the inner
        // value (`Some`'s payload) is converted as usual. `T` may be `string`, another `@json`
        // class/union, or an array of a supported element type.
        if let Some(base) = option_inner(&field.field_type) {
            if let Some(elem) = base.strip_suffix("[]") {
                let to_stmts = array_to_stmts(
                    elem,
                    &format!("__v_{}", fname),
                    &format!("__arr_{}", fname),
                    &format!("__i_{}", fname),
                    json_names,
                );
                let from_stmts = array_from_stmts(
                    elem,
                    &format!("__src_{}", fname),
                    &format!("__inner_{}", fname),
                    &format!("__i_{}", fname),
                    &format!("__asrc_{}", fname),
                    json_names,
                );
                match (to_stmts, from_stmts) {
                    (Some(to_s), Some(from_s)) => {
                        to_body.push_str(&format!(
                            "        switch (this.{f}) {{\n            Some(__v_{f}) => {{\n                {to_s}                __o.set(\"{k}\", __arr_{f});\n            }}\n            None => {{ __o.set(\"{k}\", JsonValue.none()); }}\n        }}\n",
                            f = fname, k = json_key, to_s = to_s
                        ));
                        from_prelude.push_str(&format!(
                            "        let __{f}: Option<{base}> = Option.None;\n        let __src_{f} = v.get(\"{k}\").unwrap_or(JsonValue.none());\n        if (__src_{f}.is_null() == false) {{\n            {from_s}            __{f} = Option.Some(__inner_{f});\n        }}\n",
                            f = fname, k = json_key, base = base, from_s = from_s
                        ));
                        from_fields.push(format!("__{f}", f = fname));
                    }
                    _ => {
                        diagnostics.report_error(
                            format!(
                                "@json class '{}' field '{}' has unsupported optional array element type '{}'{}",
                                name,
                                fname,
                                elem,
                                missing_json_hint(elem, jsonable)
                            ),
                            Some(field.name.position),
                        );
                        return None;
                    }
                }
                continue;
            }

            let (to_inner, from_inner) = if base == "string" {
                (
                    format!("JsonValue.from_string(__v_{f})", f = fname),
                    format!("__src_{f}.as_string()", f = fname),
                )
            } else if json_names.contains(&base) {
                (
                    format!("__v_{f}.to_json()", f = fname),
                    format!("{c}.from_json(__src_{f})", c = base, f = fname),
                )
            } else {
                diagnostics.report_error(
                    format!("@json class '{}' field '{}' has unsupported optional type '{}' (only `Option<string>`, `Option<@json class>`, and `Option<T[]>` of those are supported){}", name, fname, ftype, missing_json_hint(&base, jsonable)),
                    Some(field.name.position),
                );
                return None;
            };
            to_body.push_str(&format!(
                "        switch (this.{f}) {{\n            Some(__v_{f}) => {{ __o.set(\"{k}\", {to_inner}); }}\n            None => {{ __o.set(\"{k}\", JsonValue.none()); }}\n        }}\n",
                f = fname, k = json_key, to_inner = to_inner
            ));
            from_prelude.push_str(&format!(
                "        let __{f}: Option<{base}> = Option.None;\n        let __src_{f} = v.get(\"{k}\").unwrap_or(JsonValue.none());\n        if (__src_{f}.is_null() == false) {{\n            __{f} = Option.Some({from_inner});\n        }}\n",
                f = fname, k = json_key, base = base, from_inner = from_inner
            ));
            from_fields.push(format!("__{f}", f = fname));
            continue;
        }

        if let Some(elem) = ftype.strip_suffix("[]") {
            // Array field: serialize/deserialize element-wise. Loop variables are suffixed with the
            // field name because Dream scopes locals per-function (not per-block).
            let to_s = array_to_stmts(
                elem,
                &format!("this.{}", fname),
                &format!("__arr_{}", fname),
                &format!("__i_{}", fname),
                json_names,
            );
            let from_s = array_from_stmts(
                elem,
                &format!("v.get(\"{}\").unwrap_or(JsonValue.none())", json_key),
                &format!("__{}", fname),
                &format!("__i_{}", fname),
                &format!("__src_{}", fname),
                json_names,
            );
            match (to_s, from_s) {
                (Some(to_s), Some(from_s)) => {
                    to_body.push_str(&format!(
                        "        {to_s}        __o.set(\"{k}\", __arr_{f});\n",
                        to_s = to_s,
                        k = json_key,
                        f = fname
                    ));
                    from_prelude.push_str(&format!("        {}\n", from_s.trim_end()));
                    from_fields.push(format!("__{f}", f = fname));
                }
                _ => {
                    diagnostics.report_error(
                        format!(
                            "@json class '{}' field '{}' has unsupported array element type '{}'{}",
                            name,
                            fname,
                            elem,
                            missing_json_hint(elem, jsonable)
                        ),
                        Some(field.name.position),
                    );
                    return None;
                }
            }
        } else if is_type_param(ftype) {
            // A field typed by a generic parameter (`value: T`) is serialized through the
            // `JSON.serialize`/`JSON.deserialize` intrinsics, which the analyzer resolves per
            // concrete instantiation (`T` -> the monomorphized type's `to_json`/`from_json`). A
            // static call on the bare parameter `T` cannot be named directly, so we round-trip via
            // text: `JSON.parse(JSON.serialize(x))` yields the nested `JsonValue`.
            to_body.push_str(&format!(
                "        __o.set(\"{k}\", JSON.parse(JSON.serialize(this.{f})));\n",
                k = json_key,
                f = fname
            ));
            from_fields.push(format!(
                "JSON.deserialize<{ty}>(JSON.stringify(v.get(\"{k}\").unwrap_or(JsonValue.none())))",
                ty = ftype,
                k = json_key
            ));
        } else {
            let to_e = json_to_expr(ftype, &format!("this.{}", fname), json_names);
            let from_e = json_from_expr(
                ftype,
                &format!("v.get(\"{}\").unwrap_or(JsonValue.none())", json_key),
                json_names,
            );
            match (to_e, from_e) {
                (Some(to_e), Some(from_e)) => {
                    to_body.push_str(&format!(
                        "        __o.set(\"{k}\", {to_e});\n",
                        k = json_key,
                        to_e = to_e
                    ));
                    from_fields.push(from_e);
                }
                _ => {
                    diagnostics.report_error(
                        format!(
                            "@json class '{}' field '{}' has unsupported type '{}'{}",
                            name,
                            fname,
                            ftype,
                            missing_json_hint(ftype, jsonable)
                        ),
                        Some(field.name.position),
                    );
                    return None;
                }
            }
        }
    }
    to_body.push_str("        return __o;\n");

    // For a generic type the derive attaches to the template (`extend Box<T>`) and names the
    // instantiated type in the constructor call / return type (`Box<T>`), so each monomorphization
    // gets its own concrete `to_json`/`from_json`.
    let params_clause = if generic_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", generic_params.join(", "))
    };
    let self_ty = format!("{}{}", name, params_clause);

    let from_body = format!(
        "{prelude}        return {self_ty}({fields});\n",
        prelude = from_prelude,
        self_ty = self_ty,
        fields = from_fields.join(", ")
    );

    Some(format!(
        "extend {name}{params} {{\n    public fun to_json(): JsonValue {{\n{to_body}    }}\n    public static fun from_json(v: JsonValue): {self_ty} {{\n{from_body}    }}\n}}\n",
        name = name, params = params_clause, self_ty = self_ty, to_body = to_body, from_body = from_body
    ))
}
