//! `to_json`/`from_json` derivation for a single `@json` discriminated union (see [`super`]). Emits
//! Dream source for the `extend <Union>` converter block; values are tagged with a `"type"` key
//! naming the active variant. Re-parsed through the normal pipeline by [`super::generate_json_derives`].

use super::*;

/// Generates `extend <Union> { fun to_json(): JsonValue {...} static fun from_json(v): <Union> {...} }`
/// source for a single `@json` discriminated union, or `None` (after reporting a diagnostic) if a
/// variant payload field type is unsupported. Values are tagged internally with a `"type"` key
/// naming the active variant; unit variants serialize to `{ "type": "<Variant>" }`.
pub(super) fn generate_json_union(
    enum_decl: &dream_syntax::nodes::EnumDeclarationNode,
    json_names: &HashSet<String>,
    jsonable: &HashSet<String>,
    diagnostics: &mut DiagnosticBag,
) -> Option<String> {
    let name = &enum_decl.name.text;
    // Generic parameter names (`Result<T, E>` -> ["T", "E"]). A variant payload typed by one of
    // these round-trips through `JSON.serialize`/`JSON.deserialize`, resolved per concrete
    // instantiation by the monomorphizer (see the class path for details).
    let generic_params: Vec<String> = enum_decl
        .generic_parameters
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default();
    let is_type_param = |t: &str| generic_params.iter().any(|p| p == t);

    // `to_json`: a `switch` over the variant fills a tagged dict. Block arms run for effect.
    let mut to_body =
        String::from("        let __o = JsonValue.dict();\n        switch (this) {\n");
    // `from_json`: dispatch on the `"type"` tag, reconstructing the matching variant.
    let mut from_arms = String::new();

    for variant in &enum_decl.variants {
        let vname = &variant.name.text;
        let bindings: Vec<String> = variant.fields.iter().map(|f| f.name.text.clone()).collect();

        // to_json arm
        let pattern = if bindings.is_empty() {
            vname.clone()
        } else {
            format!("{}({})", vname, bindings.join(", "))
        };
        to_body.push_str(&format!("            {} => {{\n", pattern));
        to_body.push_str(&format!(
            "                __o.set(\"{tag}\", JsonValue.from_string(\"{v}\"));\n",
            tag = TYPE_TAG_KEY,
            v = vname
        ));
        for field in &variant.fields {
            let fname = &field.name.text;
            let ftype = field.type_token.text.as_str();
            if has_json_ignore(&field.attributes) {
                // Omitted from serialize; deserialize uses a zero default via the from_json path.
                continue;
            }
            if is_type_param(ftype) {
                to_body.push_str(&format!(
                    "                __o.set(\"{}\", JSON.parse(JSON.serialize({})));\n",
                    fname, fname
                ));
            } else if let Some(elem) = ftype.strip_suffix("[]") {
                let to_s = array_to_stmts(
                    elem,
                    fname,
                    &format!("__arr_{}_{}", vname, fname),
                    &format!("__i_{}_{}", vname, fname),
                    json_names,
                );
                match to_s {
                    Some(to_s) => {
                        // Indent each line of the array builder into the switch arm.
                        for line in to_s.lines() {
                            to_body.push_str("                ");
                            to_body.push_str(line);
                            to_body.push('\n');
                        }
                        to_body.push_str(&format!(
                            "                __o.set(\"{}\", __arr_{}_{});\n",
                            fname, vname, fname
                        ));
                    }
                    None => {
                        diagnostics.report_error(
                            format!(
                                "@json union '{}' variant '{}' field '{}' has unsupported array element type '{}'{}",
                                name,
                                vname,
                                fname,
                                elem,
                                missing_json_hint(elem, jsonable)
                            ),
                            Some(field.name.position),
                        );
                        return None;
                    }
                }
            } else {
                match json_to_expr(ftype, fname, json_names) {
                    Some(expr) => {
                        to_body.push_str(&format!(
                            "                __o.set(\"{}\", {});\n",
                            fname, expr
                        ));
                    }
                    None => {
                        diagnostics.report_error(
                            format!(
                                "@json union '{}' variant '{}' field '{}' has unsupported type '{}'{}",
                                name,
                                vname,
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
        to_body.push_str("            }\n");

        // from_json reconstruction for this variant (may need prelude stmts for array payloads).
        let ctor_block = if variant.fields.is_empty() {
            format!("            return {}.{};\n", name, vname)
        } else {
            let mut prelude = String::new();
            let mut args = Vec::new();
            for field in &variant.fields {
                let fname = &field.name.text;
                let ftype = field.type_token.text.as_str();
                if has_json_ignore(&field.attributes) {
                    match json_ignore_default(ftype, &field.field_type) {
                        Some(default) => args.push(default),
                        None => {
                            diagnostics.report_error(
                                format!(
                                    "@json union '{}' variant '{}' field '{}' has @json_ignore but type '{}' has no zero default (use `Option<{}>` or remove @json_ignore)",
                                    name, vname, fname, ftype, ftype
                                ),
                                Some(field.name.position),
                            );
                            return None;
                        }
                    }
                    continue;
                }
                let jexpr = format!("v.get(\"{}\").unwrap_or(JsonValue.none())", fname);
                if is_type_param(ftype) {
                    args.push(format!(
                        "JSON.deserialize<{}>(JSON.stringify({}))",
                        ftype, jexpr
                    ));
                } else if let Some(elem) = ftype.strip_suffix("[]") {
                    let out_var = format!("__{}_{}", vname, fname);
                    let from_s = array_from_stmts(
                        elem,
                        &jexpr,
                        &out_var,
                        &format!("__i_{}_{}", vname, fname),
                        &format!("__src_{}_{}", vname, fname),
                        json_names,
                    );
                    match from_s {
                        Some(from_s) => {
                            for line in from_s.lines() {
                                prelude.push_str("            ");
                                prelude.push_str(line);
                                prelude.push('\n');
                            }
                            args.push(out_var);
                        }
                        None => {
                            diagnostics.report_error(
                                format!(
                                    "@json union '{}' variant '{}' field '{}' has unsupported array element type '{}'{}",
                                    name, vname, fname, elem, missing_json_hint(elem, jsonable)
                                ),
                                Some(field.name.position),
                            );
                            return None;
                        }
                    }
                } else {
                    match json_from_expr(ftype, &jexpr, json_names) {
                        Some(expr) => args.push(expr),
                        None => {
                            diagnostics.report_error(
                                format!(
                                    "@json union '{}' variant '{}' field '{}' has unsupported type '{}'{}",
                                    name, vname, fname, ftype, missing_json_hint(ftype, jsonable)
                                ),
                                Some(field.name.position),
                            );
                            return None;
                        }
                    }
                }
            }
            format!(
                "{prelude}            return {name}.{vname}({args});\n",
                prelude = prelude,
                name = name,
                vname = vname,
                args = args.join(", ")
            )
        };
        from_arms.push_str(&format!(
            "        if (__t == \"{}\") {{\n{}        }}\n",
            vname, ctor_block
        ));
    }
    to_body.push_str("        }\n        return __o;\n");

    // Fallback: reconstruct the first variant for an unrecognized tag (only hit on malformed input).
    let first = &enum_decl.variants[0];
    let mut fallback_prelude = String::new();
    let fallback = if first.fields.is_empty() {
        format!("{}.{}", name, first.name.text)
    } else {
        let mut args = Vec::new();
        let vname = &first.name.text;
        for field in &first.fields {
            let fname = &field.name.text;
            let jexpr = format!("v.get(\"{}\").unwrap_or(JsonValue.none())", fname);
            let ftype = field.type_token.text.as_str();
            // Field types were already validated in the loop above.
            if has_json_ignore(&field.attributes) {
                args.push(json_ignore_default(ftype, &field.field_type)?);
            } else if is_type_param(ftype) {
                args.push(format!(
                    "JSON.deserialize<{}>(JSON.stringify({}))",
                    ftype, jexpr
                ));
            } else if let Some(elem) = ftype.strip_suffix("[]") {
                let out_var = format!("__fb_{}", fname);
                let from_s = array_from_stmts(
                    elem,
                    &jexpr,
                    &out_var,
                    &format!("__fbi_{}", fname),
                    &format!("__fbs_{}", fname),
                    json_names,
                )?;
                for line in from_s.lines() {
                    fallback_prelude.push_str("        ");
                    fallback_prelude.push_str(line);
                    fallback_prelude.push('\n');
                }
                args.push(out_var);
            } else {
                args.push(json_from_expr(ftype, &jexpr, json_names)?);
            }
        }
        format!("{}.{}({})", name, vname, args.join(", "))
    };

    let from_body = format!(
        "        let __t = v.get(\"{tag}\").unwrap_or(JsonValue.none()).as_string();\n{arms}{prelude}        return {fallback};\n",
        tag = TYPE_TAG_KEY,
        arms = from_arms,
        prelude = fallback_prelude,
        fallback = fallback
    );

    // For a generic union the derive attaches to the template (`extend Result<T, E>`) and names the
    // instantiated type in the `from_json` return type, so each monomorphization gets its own
    // concrete converters.
    let params_clause = if generic_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", generic_params.join(", "))
    };
    let self_ty = format!("{}{}", name, params_clause);

    Some(format!(
        "extend {name}{params} {{\n    public fun to_json(): JsonValue {{\n{to_body}    }}\n    public static fun from_json(v: JsonValue): {self_ty} {{\n{from_body}    }}\n}}\n",
        name = name, params = params_clause, self_ty = self_ty, to_body = to_body, from_body = from_body
    ))
}
