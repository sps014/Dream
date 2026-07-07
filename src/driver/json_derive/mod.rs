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

mod class;
mod union;

use class::generate_json_extend;
use union::generate_json_union;

/// The attribute that opts a class/union into JSON derivation.
const JSON_ATTR: &str = "json";
/// Per-field attribute overriding the emitted JSON key.
pub(super) const PROPERTY_NAME_ATTR: &str = "property_name";
/// The discriminator key written for `@json` discriminated unions.
pub(super) const TYPE_TAG_KEY: &str = "type";
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
pub(super) fn json_to_expr(
    elem_type: &str,
    access: &str,
    json_names: &HashSet<String>,
) -> Option<String> {
    Some((json_codec(elem_type, json_names)?.to)(access))
}

/// Returns the deserialize expression that reconstructs a value of `elem_type` from the JSON
/// expression `jexpr`, or `None` if the type is unsupported.
pub(super) fn json_from_expr(
    elem_type: &str,
    jexpr: &str,
    json_names: &HashSet<String>,
) -> Option<String> {
    Some((json_codec(elem_type, json_names)?.from)(jexpr))
}

/// Actionable suffix for an "unsupported field type" diagnostic: when `core` names a user-defined
/// class/struct/union that *could* be `@json` but isn't, point the user at the fix. `core` is the
/// bare type name (nullable `?` / array `[]` already stripped). Empty for genuinely unsupported
/// types (`js`, functions, C-style enums, …), which keep the plain "unsupported" wording.
pub(super) fn missing_json_hint(core: &str, jsonable: &HashSet<String>) -> String {
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
        extend_decl.is_synthesized = true;
        for method in extend_decl.methods.iter_mut() {
            // Synthesized `@json` codecs legitimately reference the user's own (possibly file-private)
            // types across the synthetic derive "file". Leaving their declaring file unset marks them
            // as compiler-synthesized so file/module-level visibility checks treat them as exempt.
            method.file_path = None;
        }
        all_extends.push(extend_decl);
    }
    Ok(())
}
