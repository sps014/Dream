//! Builtin `@json` derive: builds a declaration snapshot, runs the Dream `JsonGenerator`
//! harness (cached WASM), and `emit_file`s the resulting `extend` source.

use super::context::GeneratorContext;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::struct_node::StructDeclarationNode;
use dream_syntax::nodes::{EnumDeclarationNode, Type};
use std::collections::HashSet;
use std::io::Write;
use std::sync::OnceLock;

const HARNESS_SOURCE: &str = include_str!("json_gen_harness.dream");
const OK_MARKER: &str = "__DREAM_JSON_GEN_OK__";
const ERR_MARKER: &str = "__DREAM_JSON_GEN_ERR__";
const SNAPSHOT_ENV: &str = "DREAM_JSON_GEN_SNAPSHOT";

/// Expands every `@json` type into synthesized `extend` source through `emit_file`.
pub fn expand_from_acc(
    ctx: &mut GeneratorContext,
    structs: &[StructDeclarationNode<'_>],
    enums: &[EnumDeclarationNode<'_>],
    diagnostics: &mut DiagnosticBag,
) {
    let mut json_names: HashSet<String> = structs
        .iter()
        .filter(|s| s.attributes.iter().any(|a| a.name.text == "json"))
        .map(|s| s.name.text.clone())
        .collect();
    json_names.extend(
        enums
            .iter()
            .filter(|e| e.attributes.iter().any(|a| a.name.text == "json"))
            .map(|e| e.name.text.clone()),
    );
    if json_names.is_empty() {
        return;
    }

    #[cfg(not(feature = "native"))]
    {
        let _ = (ctx, structs, enums);
        diagnostics.report_error(
            "@json derive requires the native compiler feature (wasmtime host)".to_string(),
            None,
        );
        return;
    }

    #[cfg(feature = "native")]
    {
        let mut jsonable: HashSet<String> = structs.iter().map(|s| s.name.text.clone()).collect();
        jsonable.extend(
            enums
                .iter()
                .filter(|e| e.is_data_enum())
                .map(|e| e.name.text.clone()),
        );

        let snapshot = build_snapshot(structs, enums, &json_names, &jsonable);
        match run_dream_json_generator(&snapshot) {
            Ok(source) => {
                if !source.is_empty() {
                    ctx.emit_file("<json-derive>", source);
                }
            }
            Err(err) => {
                diagnostics.report_error(err, None);
            }
        }
    }
}

#[cfg(feature = "native")]
fn build_snapshot(
    structs: &[StructDeclarationNode<'_>],
    enums: &[EnumDeclarationNode<'_>],
    json_names: &HashSet<String>,
    jsonable: &HashSet<String>,
) -> String {
    let mut types = String::from("[");
    let mut first = true;
    for s in structs
        .iter()
        .filter(|s| s.attributes.iter().any(|a| a.name.text == "json"))
    {
        if !first {
            types.push(',');
        }
        first = false;
        types.push_str(&snapshot_class(s));
    }
    for e in enums
        .iter()
        .filter(|e| e.attributes.iter().any(|a| a.name.text == "json") && e.is_data_enum())
    {
        if !first {
            types.push(',');
        }
        first = false;
        types.push_str(&snapshot_union(e));
    }
    types.push(']');

    format!(
        "{{\"types\":{},\"json_names\":{},\"jsonable\":{}}}",
        types,
        json_string_array(json_names.iter().cloned().collect()),
        json_string_array(jsonable.iter().cloned().collect()),
    )
}

#[cfg(feature = "native")]
fn snapshot_class(s: &StructDeclarationNode<'_>) -> String {
    let generic_params: Vec<String> = s
        .generic_parameters
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default();
    let mut fields = String::from("[");
    let mut first = true;
    for field in &s.fields {
        if !first {
            fields.push(',');
        }
        first = false;
        fields.push_str(&snapshot_field(
            &field.name.text,
            field.type_token.text.as_str(),
            &field.field_type,
            &field.attributes,
            &generic_params,
        ));
    }
    fields.push(']');
    format!(
        "{{\"name\":{},\"is_union\":false,\"generic_params\":{},\"fields\":{},\"variants\":[]}}",
        json_escape(&s.name.text),
        json_string_array(generic_params),
        fields,
    )
}

#[cfg(feature = "native")]
fn snapshot_union(e: &EnumDeclarationNode<'_>) -> String {
    let generic_params: Vec<String> = e
        .generic_parameters
        .as_ref()
        .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default();
    let mut variants = String::from("[");
    let mut first_v = true;
    for variant in &e.variants {
        if !first_v {
            variants.push(',');
        }
        first_v = false;
        let mut fields = String::from("[");
        let mut first_f = true;
        for field in &variant.fields {
            if !first_f {
                fields.push(',');
            }
            first_f = false;
            fields.push_str(&snapshot_field(
                &field.name.text,
                field.type_token.text.as_str(),
                &field.field_type,
                &field.attributes,
                &generic_params,
            ));
        }
        fields.push(']');
        variants.push_str(&format!(
            "{{\"name\":{},\"fields\":{}}}",
            json_escape(&variant.name.text),
            fields
        ));
    }
    variants.push(']');
    format!(
        "{{\"name\":{},\"is_union\":true,\"generic_params\":{},\"fields\":[],\"variants\":{}}}",
        json_escape(&e.name.text),
        json_string_array(generic_params),
        variants,
    )
}

#[cfg(feature = "native")]
fn snapshot_field(
    name: &str,
    type_name: &str,
    field_ty: &Type,
    attrs: &[dream_syntax::nodes::AttributeNode],
    generic_params: &[String],
) -> String {
    let json_ignore = attrs.iter().any(|a| a.name.text == "json_ignore");
    let mut property_name = String::new();
    if let Some(prop) = attrs.iter().find(|a| a.name.text == "property_name") {
        if let Some(arg) = prop.args.first() {
            property_name = arg.text.trim_matches('"').to_string();
        }
    }
    let option_inner = match field_ty {
        Type::Struct(token, Some(args)) if token.text == "Option" && args.len() == 1 => {
            args[0].get_type()
        }
        _ => String::new(),
    };
    let is_type_param = generic_params.iter().any(|p| p == type_name);
    format!(
        "{{\"name\":{},\"type_name\":{},\"json_ignore\":{},\"property_name\":{},\"option_inner\":{},\"is_type_param\":{}}}",
        json_escape(name),
        json_escape(type_name),
        if json_ignore { "true" } else { "false" },
        json_escape(&property_name),
        json_escape(&option_inner),
        if is_type_param { "true" } else { "false" },
    )
}

#[cfg(feature = "native")]
fn json_string_array(mut items: Vec<String>) -> String {
    // Deterministic order for reproducible harness input (not required for correctness).
    items.sort();
    let mut out = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_escape(s));
    }
    out.push(']');
    out
}

#[cfg(feature = "native")]
fn json_escape(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(feature = "native")]
fn run_dream_json_generator(snapshot: &str) -> Result<String, String> {
    let wat_path = harness_wat_path()?;
    let mut snap_file = snap_tempfile()?;
    snap_file
        .write_all(snapshot.as_bytes())
        .map_err(|e| format!("@json generator: failed to write snapshot: {e}"))?;
    let snap_path = snap_file.path.to_string_lossy().into_owned();

    std::env::set_var(SNAPSHOT_ENV, &snap_path);
    let output = crate::execution::wasm_runner::execute_wasm_capturing(&wat_path)
        .map_err(|e| format!("@json generator: failed to run Dream harness: {e}"))?;
    let _ = std::env::var(SNAPSHOT_ENV);
    std::env::remove_var(SNAPSHOT_ENV);
    drop(snap_file);

    parse_generator_output(&output)
}

#[cfg(feature = "native")]
fn parse_generator_output(output: &str) -> Result<String, String> {
    let trimmed = output.trim_start();
    if let Some(rest) = trimmed.strip_prefix(OK_MARKER) {
        let source = rest.strip_prefix('\n').unwrap_or(rest);
        return Ok(source.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix(ERR_MARKER) {
        let msg = rest.trim();
        return Err(if msg.is_empty() {
            "@json generator failed".to_string()
        } else {
            msg.to_string()
        });
    }
    Err(format!(
        "@json generator: unexpected harness output (missing OK/ERR marker):\n{output}"
    ))
}

#[cfg(feature = "native")]
fn harness_wat_path() -> Result<String, String> {
    static HARNESS: OnceLock<Result<String, String>> = OnceLock::new();
    HARNESS
        .get_or_init(|| {
            // Fingerprint harness + generator Dream sources so a stdlib edit invalidates the cache.
            let fingerprint = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                HARNESS_SOURCE.hash(&mut h);
                include_str!("../../../crates/dream-stdlib/src/system/json/json_generator.dream")
                    .hash(&mut h);
                include_str!("../../../crates/dream-stdlib/src/system/json/json_gen_model.dream")
                    .hash(&mut h);
                h.finish()
            };
            let dir = std::env::temp_dir().join(format!("dream-json-gen-harness-{fingerprint:x}"));
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("@json generator: create harness dir: {e}"))?;
            let src_path = dir.join("harness.dream");
            let wat_path = dir.join("harness.wat");
            if wat_path.is_file() {
                return Ok(wat_path.to_string_lossy().into_owned());
            }
            std::fs::write(&src_path, HARNESS_SOURCE)
                .map_err(|e| format!("@json generator: write harness source: {e}"))?;
            let src = src_path.to_string_lossy().into_owned();
            let out = wat_path.to_string_lossy().into_owned();
            let compiler = crate::driver::compiler::Compiler::new(
                crate::driver::compiler::Target::Wasm,
            )
            .with_skip_generators(true)
            .with_release(true);
            compiler
                .compile(&src, &out)
                .map_err(|e| format!("@json generator: failed to compile Dream harness: {e:?}"))?;
            Ok(out)
        })
        .clone()
}

#[cfg(feature = "native")]
struct SnapTempFile {
    path: std::path::PathBuf,
    file: std::fs::File,
}

#[cfg(feature = "native")]
impl Write for SnapTempFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(feature = "native")]
impl Drop for SnapTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(feature = "native")]
fn snap_tempfile() -> Result<SnapTempFile, String> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "dream-json-gen-snap-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("@json generator: create snapshot file: {e}"))?;
    Ok(SnapTempFile { path, file })
}
