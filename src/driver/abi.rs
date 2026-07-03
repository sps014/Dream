use std::fs;
use std::io::Error;
use std::path::Path;
use tracing::{error, info};

use crate::syntax::nodes::ProgramNode;

/// Emits a binary `.wasm` next to the `.wat`, plus an `.abi.json` describing the module's
/// extern imports (for JS interop marshaling) and exported functions.
pub(crate) fn emit_wasm_and_abi(
    wat_path: &str,
    wat_text: &str,
    program: &ProgramNode,
) -> Result<(), Error> {
    let base = Path::new(wat_path);

    let wasm_path = base.with_extension("wasm");
    match wat::parse_str(wat_text) {
        Ok(bytes) => {
            fs::write(&wasm_path, bytes)?;
            info!("created file: {}", wasm_path.display());
        }
        Err(e) => {
            // Non-fatal: the `.wat` is still valid output; just warn.
            error!("could not assemble binary wasm: {}", e);
        }
    }

    let abi_path = base.with_extension("abi.json");
    fs::write(&abi_path, build_abi_json(program))?;
    info!("created file: {}", abi_path.display());
    Ok(())
}

/// Escapes a string for embedding in a JSON document.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Builds the `.abi.json` describing extern imports and exported functions. The JS runtime uses
/// this to wrap user-supplied import implementations with the correct value marshaling.
pub(crate) fn build_abi_json(program: &ProgramNode) -> String {
    fn type_name(t: Option<&crate::syntax::nodes::Type>) -> String {
        match t {
            Some(t) => t.get_type(),
            None => "void".to_string(),
        }
    }

    // Emits one extern's ABI entry, or `None` for non-externs and `@intrinsic` declarations (which
    // are lowered by the compiler and emit no WASM import). Shared by top-level functions and class
    // / `extend` static externs so a host bridge keeps its ABI entry wherever it is declared.
    fn extern_entry(func: &crate::syntax::nodes::FunctionNode) -> Option<String> {
        if !func.is_extern || crate::intrinsics::has_intrinsic_attr(&func.attributes) {
            return None;
        }
        let mut import_module = crate::mir::abi::ENV_MODULE.to_string();
        let mut import_name = func.name.text.clone();
        if let Some(js_attr) = func.attributes.iter().find(|a| a.name.text == "js") {
            if let Some(arg) = js_attr.args.first() {
                import_module = arg.text.trim_matches('"').to_string();
            }
            if let Some(arg) = js_attr.args.get(1) {
                import_name = arg.text.trim_matches('"').to_string();
            }
        }
        let params: Vec<String> = func
            .parameters
            .iter()
            .map(|p| format!("\"{}\"", json_escape(&p.type_.get_type())))
            .collect();
        Some(format!(
            "    {{ \"name\": \"{}\", \"module\": \"{}\", \"field\": \"{}\", \"params\": [{}], \"result\": \"{}\", \"async\": {} }}",
            json_escape(&func.name.text),
            json_escape(&import_module),
            json_escape(&import_name),
            params.join(", "),
            json_escape(&type_name(func.return_type.as_ref())),
            func.is_async,
        ))
    }

    // Walk top-level externs plus class (`structs[].methods`) and `extend` (`extends[].methods`)
    // static externs, so host bridges moved onto their owning class still appear in the sidecar.
    let mut externs = Vec::new();
    let class_methods = program.structs.iter().flat_map(|s| s.methods.iter());
    let extend_methods = program.extends.iter().flat_map(|e| e.methods.iter());
    for func in program
        .functions
        .iter()
        .chain(class_methods)
        .chain(extend_methods)
    {
        if let Some(entry) = extern_entry(func) {
            externs.push(entry);
        }
    }

    let mut exports = Vec::new();
    for func in program.functions.iter() {
        if func.is_extern || func.generic_parameters.is_some() {
            continue;
        }
        if func.is_public || func.name.text == crate::mir::abi::ENTRY_FN {
            exports.push(format!("\"{}\"", json_escape(&func.name.text)));
        }
    }

    format!(
        "{{\n  \"externs\": [\n{}\n  ],\n  \"exports\": [{}]\n}}\n",
        externs.join(",\n"),
        exports.join(", "),
    )
}
