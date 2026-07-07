//! `to_json`/`from_json` derivation for a single `@json` class (see [`super`]). Emits Dream source
//! for the `extend <Class>` converter block; re-parsed through the normal pipeline by
//! [`super::generate_json_derives`].

use super::*;

/// Generates `extend <Class> { fun to_json(): JsonValue {...} static fun from_json(v): <Class> {...} }`
/// source for a single `@json` class, or `None` (after reporting a diagnostic) if a field type is
/// outside the supported set (primitives, `string`, other `@json` classes, and arrays of those).
pub(super) fn generate_json_extend(
    struct_decl: &crate::syntax::nodes::struct_node::StructDeclarationNode,
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

        // Nullable field (`T?`): a JSON `null` maps to/from the Dream `null`, otherwise the inner
        // value is converted as usual. Only reference types can be nullable in Dream, so the inner
        // type is `string` or another `@json` class (nullable arrays are out of scope).
        if let Some(base) = ftype.strip_suffix('?') {
            let (to_inner, from_inner) = if base == "string" {
                (
                    format!("JsonValue.from_string(this.{f} ?? \"\")", f = fname),
                    format!("__src_{f}.as_string()", f = fname),
                )
            } else if json_names.contains(base) {
                (
                    format!("this.{f}.to_json()", f = fname),
                    format!("{c}.from_json(__src_{f})", c = base, f = fname),
                )
            } else {
                diagnostics.report_error(
                    format!("@json class '{}' field '{}' has unsupported nullable type '{}' (only `string?` and nullable @json classes are supported){}", name, fname, ftype, missing_json_hint(base, jsonable)),
                    Some(field.name.position),
                );
                return None;
            };
            to_body.push_str(&format!(
                "        if (this.{f} == null) {{\n            __o.set(\"{k}\", JsonValue.none());\n        }} else {{\n            __o.set(\"{k}\", {to_inner});\n        }}\n",
                f = fname, k = json_key, to_inner = to_inner
            ));
            from_prelude.push_str(&format!(
                "        let __{f}: {ty} = null;\n        let __src_{f} = v.get(\"{k}\").unwrap_or(JsonValue.none());\n        if (__src_{f}.is_null() == false) {{\n            __{f} = {from_inner};\n        }}\n",
                f = fname, k = json_key, ty = ftype, from_inner = from_inner
            ));
            from_fields.push(format!("__{f}", f = fname));
            continue;
        }

        if let Some(elem) = ftype.strip_suffix("[]") {
            // Array field: serialize/deserialize element-wise. Loop variables are suffixed with the
            // field name because Dream scopes locals per-function (not per-block).
            let to_elem = json_to_expr(elem, &format!("this.{}[__i_{}]", fname, fname), json_names);
            let from_elem = json_from_expr(
                elem,
                &format!(
                    "__src_{}.at(__i_{}).unwrap_or(JsonValue.none())",
                    fname, fname
                ),
                json_names,
            );
            match (to_elem, from_elem) {
                (Some(to_e), Some(from_e)) => {
                    to_body.push_str(&format!(
                        "        let __arr_{f} = JsonValue.array();\n        let __i_{f} = 0;\n        while (__i_{f} < this.{f}.size()) {{\n            __arr_{f}.push({to_e});\n            __i_{f} = __i_{f} + 1;\n        }}\n        __o.set(\"{k}\", __arr_{f});\n",
                        f = fname, k = json_key, to_e = to_e
                    ));
                    from_prelude.push_str(&format!(
                        "        let __src_{f} = v.get(\"{k}\").unwrap_or(JsonValue.none());\n        let __{f} = Buffer.alloc<{elem}>(__src_{f}.size());\n        let __i_{f} = 0;\n        while (__i_{f} < __src_{f}.size()) {{\n            __{f}[__i_{f}] = {from_e};\n            __i_{f} = __i_{f} + 1;\n        }}\n",
                        f = fname, k = json_key, elem = elem, from_e = from_e
                    ));
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
