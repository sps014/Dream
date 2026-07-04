//! `@json` derive support: generates `to_json`/`from_json` `extend` blocks for `@json`-annotated
//! classes and discriminated unions. The strategy is to emit Dream source for the converters and
//! re-parse it (so the generated methods go through the normal analyzer/codegen path); an
//! AST-based derive is noted as a future option.

use bumpalo::Bump;
use std::collections::{HashMap, HashSet};
use std::io::Error;

use crate::diagnostics::DiagnosticBag;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;

/// The attribute that opts a class/union into JSON derivation.
const JSON_ATTR: &str = "json";
/// Per-field attribute overriding the emitted JSON key.
const PROPERTY_NAME_ATTR: &str = "property_name";
/// The discriminator key written for `@json` discriminated unions.
const TYPE_TAG_KEY: &str = "type";
/// Synthetic file name under which the generated derive source is parsed/reported.
const JSON_DERIVE_FILE: &str = "<json-derive>";

/// One primitive's JSON codec: how to serialize (`to`, given the accessor expression) and
/// deserialize (`from`, given the source `JsonValue` expression). Unifies the former parallel
/// serialize/deserialize maps into a single table.
struct JsonCodec {
    to: Box<dyn Fn(&str) -> String>,
    from: Box<dyn Fn(&str) -> String>,
}

/// Returns the [`JsonCodec`] for a field element type, or `None` if the type is outside the
/// supported set (primitives, `string`, and other `@json` classes/unions).
fn json_codec(elem_type: &str, json_names: &HashSet<String>) -> Option<JsonCodec> {
    let codec = match elem_type {
        "int" => JsonCodec {
            to: Box::new(|a| format!("JsonValue.from_int({})", a)),
            from: Box::new(|j| format!("{}.as_int()", j)),
        },
        "double" => JsonCodec {
            to: Box::new(|a| format!("JsonValue.number({})", a)),
            from: Box::new(|j| format!("{}.as_double()", j)),
        },
        "float" => JsonCodec {
            to: Box::new(|a| format!("JsonValue.number((double){})", a)),
            from: Box::new(|j| format!("(float){}.as_double()", j)),
        },
        "bool" => JsonCodec {
            to: Box::new(|a| format!("JsonValue.boolean({})", a)),
            from: Box::new(|j| format!("{}.as_bool()", j)),
        },
        "string" => JsonCodec {
            to: Box::new(|a| format!("JsonValue.from_string({})", a)),
            from: Box::new(|j| format!("{}.as_string()", j)),
        },
        c if json_names.contains(c) => {
            let cls = c.to_string();
            JsonCodec {
                to: Box::new(|a| format!("{}.to_json()", a)),
                from: Box::new(move |j| format!("{}.from_json({})", cls, j)),
            }
        }
        _ => return None,
    };
    Some(codec)
}

/// Classifies a field's element type for JSON derivation, returning the serialize expression for
/// `access`, or `None` if the type is unsupported.
fn json_to_expr(elem_type: &str, access: &str, json_names: &HashSet<String>) -> Option<String> {
    Some((json_codec(elem_type, json_names)?.to)(access))
}

/// Returns the deserialize expression that reconstructs a value of `elem_type` from the JSON
/// expression `jexpr`, or `None` if the type is unsupported.
fn json_from_expr(elem_type: &str, jexpr: &str, json_names: &HashSet<String>) -> Option<String> {
    Some((json_codec(elem_type, json_names)?.from)(jexpr))
}

/// Actionable suffix for an "unsupported field type" diagnostic: when `core` names a user-defined
/// class/struct/union that *could* be `@json` but isn't, point the user at the fix. `core` is the
/// bare type name (nullable `?` / array `[]` already stripped). Empty for genuinely unsupported
/// types (`js`, functions, C-style enums, …), which keep the plain "unsupported" wording.
fn missing_json_hint(core: &str, jsonable: &HashSet<String>) -> String {
    if jsonable.contains(core) {
        format!(
            "; '{core}' must itself be marked @json (add the @json attribute to it)",
            core = core
        )
    } else {
        String::new()
    }
}

/// Returns `true` if the declaration carries the `@json` attribute.
fn has_json_attr<'a>(
    attributes: impl IntoIterator<Item = &'a crate::syntax::nodes::AttributeNode>,
) -> bool {
    attributes.into_iter().any(|a| a.name.text == JSON_ATTR)
}

/// Generates `extend <Class> { fun to_json(): JsonValue {...} static fun from_json(v): <Class> {...} }`
/// source for a single `@json` class, or `None` (after reporting a diagnostic) if a field type is
/// outside the supported set (primitives, `string`, other `@json` classes, and arrays of those).
fn generate_json_extend(
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
                        "        let __src_{f} = v.get(\"{k}\").unwrap_or(JsonValue.none());\n        let __{f} = Array.new<{elem}>(__src_{f}.size());\n        let __i_{f} = 0;\n        while (__i_{f} < __src_{f}.size()) {{\n            __{f}[__i_{f}] = {from_e};\n            __i_{f} = __i_{f} + 1;\n        }}\n",
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

/// Generates `extend <Union> { fun to_json(): JsonValue {...} static fun from_json(v): <Union> {...} }`
/// source for a single `@json` discriminated union, or `None` (after reporting a diagnostic) if a
/// variant payload field type is unsupported. Values are tagged internally with a `"type"` key
/// naming the active variant; unit variants serialize to `{ "type": "<Variant>" }`.
fn generate_json_union(
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

/// For every `@json` class and discriminated union, generates and parses its `to_json`/`from_json`
/// converter `extend` block and appends the methods to `all_extends`. Runs after all user/prelude
/// declarations are collected so cross-type (`@json` field) references resolve.
pub(crate) fn generate_json_derives<'a>(
    arena: &'a Bump,
    all_structs: &[crate::syntax::nodes::struct_node::StructDeclarationNode<'a>],
    all_enums: &[crate::syntax::nodes::EnumDeclarationNode],
    all_extends: &mut Vec<crate::syntax::nodes::ExtendNode<'a>>,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
) -> Result<(), Error> {
    let mut json_names: HashSet<String> = all_structs
        .iter()
        .filter(|s| has_json_attr(&s.attributes))
        .map(|s| s.name.text.clone())
        .collect();
    // `@json` discriminated unions participate too, so nested `@json` fields can reference them.
    json_names.extend(
        all_enums
            .iter()
            .filter(|e| has_json_attr(&e.attributes))
            .map(|e| e.name.text.clone()),
    );
    if json_names.is_empty() {
        return Ok(());
    }

    // User-defined types that *could* be `@json` (every class/struct, plus discriminated unions).
    // Used to turn an "unsupported field type" error into an actionable "mark it @json" hint. Plain
    // C-style enums are excluded — `@json` isn't supported on them.
    let mut jsonable: HashSet<String> = all_structs.iter().map(|s| s.name.text.clone()).collect();
    jsonable.extend(
        all_enums
            .iter()
            .filter(|e| e.is_data_enum())
            .map(|e| e.name.text.clone()),
    );

    let mut source = String::new();
    for struct_decl in all_structs.iter().filter(|s| has_json_attr(&s.attributes)) {
        if let Some(block) = generate_json_extend(struct_decl, &json_names, &jsonable, diagnostics)
        {
            source.push_str(&block);
            source.push('\n');
        }
    }

    for enum_decl in all_enums.iter().filter(|e| has_json_attr(&e.attributes)) {
        if !enum_decl.is_data_enum() {
            diagnostics.report_error(
                format!(
                    "@json is only supported on discriminated unions, not the plain enum '{}'",
                    enum_decl.name.text
                ),
                Some(enum_decl.name.position),
            );
            continue;
        }
        if let Some(block) = generate_json_union(enum_decl, &json_names, &jsonable, diagnostics) {
            source.push_str(&block);
            source.push('\n');
        }
    }

    if source.is_empty() {
        return Ok(());
    }

    let prelude_name = JSON_DERIVE_FILE.to_string();
    file_contents.insert(prelude_name.clone(), source.clone());
    let mut derive_diagnostics = DiagnosticBag::new(Some(prelude_name.clone()));
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer, arena, &mut derive_diagnostics);
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(e) => {
            diagnostics.extend(&derive_diagnostics);
            return Err(e);
        }
    };
    diagnostics.extend(&derive_diagnostics);

    let program = ast.get_root();
    let file_tag: std::rc::Rc<str> = std::rc::Rc::from(prelude_name.as_str());
    for extend_decl in program.extends.iter().cloned() {
        let mut extend_decl = extend_decl;
        extend_decl.file_path = Some(file_tag.clone());
        for method in extend_decl.methods.iter_mut() {
            method.file_path = Some(file_tag.clone());
        }
        all_extends.push(extend_decl);
    }
    Ok(())
}
