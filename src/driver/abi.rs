use std::collections::BTreeSet;
use std::fs;
use std::io::Error;
use std::path::Path;
use tracing::{error, info};

use crate::driver::compute_gen::{self, GpuKernelInfo};
use dream_syntax::nodes::ProgramNode;

/// One live host import after MIR pruning: `(module, field)` as emitted on the WASM import.
pub type LiveImport = (String, String);

/// Emits a binary `.wasm` next to the `.wat`, plus an `.abi.json` describing the module's
/// **live** extern imports (for JS interop marshaling) and exported functions. When `kernels` is
/// non-empty, also writes a sibling `.wgsl` file and embeds a `"gpu"` section in the ABI.
pub(crate) fn emit_wasm_and_abi(
    wat_path: &str,
    wat_text: &str,
    program: &ProgramNode,
    kernels: &[GpuKernelInfo],
    live_imports: &[LiveImport],
) -> Result<(), Error> {
    let base = Path::new(wat_path);

    let wasm_path = base.with_extension("wasm");
    match wat::parse_str(wat_text) {
        Ok(bytes) => {
            fs::write(&wasm_path, bytes)?;
            info!("created file: {}", wasm_path.display());
        }
        Err(e) => {
            error!("could not assemble binary wasm: {}", e);
        }
    }

    if !kernels.is_empty() {
        let wgsl_path = base.with_extension("wgsl");
        fs::write(&wgsl_path, compute_gen::join_wgsl_module(kernels))?;
        info!("created file: {}", wgsl_path.display());
    }

    let abi_path = base.with_extension("abi.json");
    fs::write(&abi_path, build_abi_json(program, kernels, live_imports))?;
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

/// Builds the `.abi.json` describing live extern imports and exported functions. Externs are
/// taken from the AST (for accurate Dream names / async flags / type strings) but filtered to
/// `(module, field)` pairs that survived MIR import pruning.
pub(crate) fn build_abi_json(
    program: &ProgramNode,
    kernels: &[GpuKernelInfo],
    live_imports: &[LiveImport],
) -> String {
    let live: BTreeSet<(&str, &str)> = live_imports
        .iter()
        .map(|(m, f)| (m.as_str(), f.as_str()))
        .collect();

    fn type_name(t: Option<&dream_syntax::nodes::Type>) -> String {
        match t {
            Some(t) => t.get_type(),
            None => "void".to_string(),
        }
    }

    fn extern_entry(func: &dream_syntax::nodes::FunctionNode) -> Option<(String, String, String)> {
        if !func.is_extern || dream_abi::intrinsics::has_intrinsic_attr(&func.attributes) {
            return None;
        }
        let (import_module, import_name) = dream_abi::attributes::js_import_target(&func.attributes)
            .unwrap_or_else(|| {
                (
                    dream_mir::abi::ENV_MODULE.to_string(),
                    func.name.text.clone(),
                )
            });
        let params: Vec<String> = func
            .parameters
            .iter()
            .map(|p| format!("\"{}\"", json_escape(&p.type_.get_type())))
            .collect();
        let entry = format!(
            "    {{ \"name\": \"{}\", \"module\": \"{}\", \"field\": \"{}\", \"params\": [{}], \"result\": \"{}\", \"async\": {} }}",
            json_escape(&func.name.text),
            json_escape(&import_module),
            json_escape(&import_name),
            params.join(", "),
            json_escape(&type_name(func.return_type.as_ref())),
            func.is_async,
        );
        Some((import_module, import_name, entry))
    }

    let mut externs = Vec::new();
    let mut seen_fields: BTreeSet<(String, String)> = BTreeSet::new();
    let class_methods = program.structs.iter().flat_map(|s| s.methods.iter());
    let extend_methods = program.extends.iter().flat_map(|e| e.methods.iter());
    for func in program
        .functions
        .iter()
        .chain(class_methods)
        .chain(extend_methods)
    {
        if let Some((module, field, entry)) = extern_entry(func) {
            if !live.contains(&(module.as_str(), field.as_str())) {
                continue;
            }
            if !seen_fields.insert((module, field)) {
                continue;
            }
            externs.push(entry);
        }
    }

    let mut exports = Vec::new();
    for func in program.functions.iter() {
        if func.is_extern || func.generic_parameters.is_some() {
            continue;
        }
        if dream_abi::attributes::has_compute_attr(&func.attributes) {
            continue;
        }
        if func.visibility.is_public() || func.name.text == dream_mir::abi::ENTRY_FN {
            exports.push(format!("\"{}\"", json_escape(&func.name.text)));
        }
    }

    let gpu_section = if kernels.is_empty() {
        String::new()
    } else {
        format!(",\n  \"gpu\": {{ {} }}", compute_gen::gpu_abi_json(kernels))
    };

    format!(
        "{{\n  \"externs\": [\n{}\n  ],\n  \"exports\": [{}]{}\n}}\n",
        externs.join(",\n"),
        exports.join(", "),
        gpu_section,
    )
}
