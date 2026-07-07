//! `to_json`/`from_json` derivation for a single `@json` discriminated union (see [`super`]). Emits
//! Dream source for the `extend <Union>` converter block; values are tagged with a `"type"` key
//! naming the active variant. Re-parsed through the normal pipeline by [`super::generate_json_derives`].

use super::*;

/// Generates `extend <Union> { fun to_json(): JsonValue {...} static fun from_json(v): <Union> {...} }`
/// source for a single `@json` discriminated union, or `None` (after reporting a diagnostic) if a
/// variant payload field type is unsupported. Values are tagged internally with a `"type"` key
/// naming the active variant; unit variants serialize to `{ "type": "<Variant>" }`.
pub(super) fn generate_json_union(
    enum_decl: &crate::syntax::nodes::EnumDeclarationNode,
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
            let to_expr = if is_type_param(ftype) {
                Some(format!("JSON.parse(JSON.serialize({}))", fname))
            } else {
                json_to_expr(ftype, fname, json_names)
            };
            match to_expr {
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
        to_body.push_str("            }\n");

        // from_json reconstruction expression for this variant
        let ctor = if variant.fields.is_empty() {
            format!("{}.{}", name, vname)
        } else {
            let mut args = Vec::new();
            for field in &variant.fields {
                let fname = &field.name.text;
                let ftype = field.type_token.text.as_str();
                let jexpr = format!("v.get(\"{}\").unwrap_or(JsonValue.none())", fname);
                let from_expr = if is_type_param(ftype) {
                    Some(format!(
                        "JSON.deserialize<{}>(JSON.stringify({}))",
                        ftype, jexpr
                    ))
                } else {
                    json_from_expr(ftype, &jexpr, json_names)
                };
                match from_expr {
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
            format!("{}.{}({})", name, vname, args.join(", "))
        };
        from_arms.push_str(&format!(
            "        if (__t == \"{}\") {{\n            return {};\n        }}\n",
            vname, ctor
        ));
    }
    to_body.push_str("        }\n        return __o;\n");

    // Fallback: reconstruct the first variant for an unrecognized tag (only hit on malformed input).
    let first = &enum_decl.variants[0];
    let fallback = if first.fields.is_empty() {
        format!("{}.{}", name, first.name.text)
    } else {
        let mut args = Vec::new();
        for field in &first.fields {
            let jexpr = format!("v.get(\"{}\").unwrap_or(JsonValue.none())", field.name.text);
            let ftype = field.type_token.text.as_str();
            // Field types were already validated in the loop above.
            if is_type_param(ftype) {
                args.push(format!(
                    "JSON.deserialize<{}>(JSON.stringify({}))",
                    ftype, jexpr
                ));
            } else {
                args.push(json_from_expr(ftype, &jexpr, json_names)?);
            }
        }
        format!("{}.{}({})", name, first.name.text, args.join(", "))
    };

    let from_body = format!(
        "        let __t = v.get(\"{tag}\").unwrap_or(JsonValue.none()).as_string();\n{arms}        return {fallback};\n",
        tag = TYPE_TAG_KEY,
        arms = from_arms,
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
