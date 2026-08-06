//! Selective JS runtime emission: assemble a tree-shaken `*.runtime.js` next to each `.wasm`
//! from the modular sources under `runtime/src/`, including only the host chunks required by
//! live WASM imports.

use std::collections::BTreeSet;
use std::fs;
use std::io::Error;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::driver::abi::LiveImport;

const ALWAYS_MODULES: &[&str] = &[
    "platform.js",
    "core.js",
    "instance.js",
    "marshal.js",
    "hosts/env.js",
];

/// Map a Dream host import field to the optional chunk that implements it.
fn chunk_for_field(field: &str) -> Option<&'static str> {
    if field.starts_with("js") {
        Some("js")
    } else if field.starts_with("http") {
        Some("http")
    } else if field.starts_with("file") || field.starts_with("dir") {
        Some("fs")
    } else if field.starts_with("crypto") {
        Some("crypto")
    } else if field.starts_with("gpu") {
        Some("gpu")
    } else if field.starts_with("worker") {
        Some("workers")
    } else if field.starts_with("console") || field.starts_with("process") {
        Some("console_process")
    } else if field.starts_with("unicode")
        || field.starts_with("date")
        || field.starts_with("time")
        || field == "delayMs"
    {
        Some("datetime_text")
    } else {
        None
    }
}

fn chunk_file(chunk: &str) -> Option<&'static str> {
    match chunk {
        "js" => Some("hosts/js.js"),
        "http" => Some("hosts/http.js"),
        "fs" => Some("hosts/fs.js"),
        "crypto" => Some("hosts/crypto.js"),
        "gpu" => Some("hosts/gpu.js"),
        "console_process" => Some("hosts/console_process.js"),
        "datetime_text" => Some("hosts/datetime_text.js"),
        "workers" => Some("workers.js"),
        _ => None,
    }
}

fn runtime_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/src")
}

/// Strip ESM imports / rewrite exports so modules can share one scope when concatenated.
fn transform_module(text: &str, rel: &str) -> String {
    let mut cleaned = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ") {
            continue;
        }
        if trimmed.starts_with("export {") || trimmed.starts_with("export default") {
            continue;
        }
        let mut line = line.to_string();
        for (from, to) in [
            ("export async function ", "async function "),
            ("export function ", "function "),
            ("export class ", "class "),
            ("export const ", "const "),
            ("export let ", "let "),
        ] {
            if let Some(rest) = line.trim_start().strip_prefix(from) {
                let indent_len = line.len() - line.trim_start().len();
                line = format!("{}{}{}", &line[..indent_len], to, rest);
                break;
            }
        }
        cleaned.push_str(&line);
        cleaned.push('\n');
    }

    format!("\n// ----- {} -----\n{}", rel, cleaned.trim())
}

fn factory_spread(chunks: &BTreeSet<&str>) -> String {
    let mut parts = Vec::new();
    if chunks.contains("gpu") {
        parts.push("    ...makeGpuHost(getInstance),");
    }
    if chunks.contains("js") {
        parts.push("    ...makeJsHost(getInstance),");
    }
    if chunks.contains("http") {
        parts.push("    ...makeHttpHost(),");
    }
    if chunks.contains("fs") {
        parts.push("    ...makeFsHost(),");
    }
    if chunks.contains("crypto") {
        parts.push("    ...makeCryptoHost(),");
    }
    if chunks.contains("datetime_text") {
        parts.push("    ...makeDatetimeTextHost(),");
    }
    if chunks.contains("console_process") {
        parts.push("    ...makeConsoleProcessHost(),");
    }
    if parts.is_empty() {
        "  return {};".to_string()
    } else {
        format!("  return {{\n{}\n  }};", parts.join("\n"))
    }
}

fn load_footer(chunks: &BTreeSet<&str>) -> String {
    let need_crypto = chunks.contains("crypto");
    let need_workers = chunks.contains("workers");
    let need_fs = chunks.contains("fs") || chunks.contains("console_process");
    let compose = factory_spread(chunks);

    let crypto_preload = if need_crypto {
        r#"
  if (isNode) {
    try { setNodeCrypto(await import("node:crypto")); } catch (_) {}
  }
"#
    } else {
        ""
    };
    let fs_preload = if need_fs {
        r#"
  if (isNode) {
    try { setNodeFs(await import("node:fs")); } catch (_) {}
  }
"#
    } else {
        ""
    };

    let workers_spread = if need_workers {
        "    ...makeWorkerModule(wasmBytes, abi, () => sharedMemory),\n"
    } else {
        ""
    };

    let crypto_wrap = if need_crypto {
        r#"
    if (sig && sig.field === "cryptoSecureRandomFill") {
      return wrapInPlaceByteArrayFill(getInstance, (count) => csprngBytes(count));
    }
"#
    } else {
        ""
    };

    format!(
        r#"
function defaultDreamModule(getInstance) {{
{compose}
}}

const FALLBACK_INITIAL_MEMORY_PAGES = 64;
const FALLBACK_MAX_MEMORY_PAGES = 65536;

async function fetchBytes(source) {{
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  if (source instanceof Uint8Array) return source;
  if (!isNode && typeof fetch === "function") {{
    const res = await fetch(source);
    if (!res.ok) throw new Error(`failed to fetch ${{source}}: ${{res.status}}`);
    return new Uint8Array(await res.arrayBuffer());
  }}
  const {{ readFile }} = await import("node:fs/promises");
  return new Uint8Array(await readFile(source));
}}

async function loadAbi(abi) {{
  if (!abi) return null;
  if (typeof abi === "object" && abi.externs) return abi;
  const bytes = await fetchBytes(abi);
  return JSON.parse(new TextDecoder("utf-8").decode(bytes));
}}

function makeLinearMemory(wasmModule) {{
  const memoryImport = WebAssembly.Module.imports(wasmModule).find(
    (i) => i.module === "env" && i.name === "memory" && i.kind === "memory",
  );
  const desc = memoryImport && memoryImport.type;
  return new WebAssembly.Memory({{
    initial: desc ? desc.minimum : FALLBACK_INITIAL_MEMORY_PAGES,
    maximum: desc ? desc.maximum : FALLBACK_MAX_MEMORY_PAGES,
    shared: desc ? desc.shared : true,
  }});
}}

async function load(source, options = {{}}) {{
  const wasmBytes = await fetchBytes(source);
  const abi = await loadAbi(options.abi);
  const wasmModule = await WebAssembly.compile(wasmBytes);
{fs_preload}{crypto_preload}
  let instance = null;
  const getInstance = () => {{
    if (!instance) throw new Error("instance not ready");
    return instance;
  }};

  const importObject = {{ env: defaultEnv(getInstance, options) }};
  const sharedMemory = options.memory ?? makeLinearMemory(wasmModule);
  importObject.env.memory = sharedMemory;

  const userImports = options.imports || {{}};
  const sigByName = new Map();
  if (abi) for (const e of abi.externs) sigByName.set(e.name, e);

  const builtinDream = {{
    ...defaultDreamModule(getInstance),
{workers_spread}  }};
  if (typeof builtinDream.__attachGpuAbi === "function") {{
    const hint =
      typeof source === "string"
        ? source
        : typeof options.abi === "string"
          ? options.abi
          : null;
    builtinDream.__attachGpuAbi(abi, hint);
  }}

  const wrapFor = (fn, sig) => {{
{crypto_wrap}    return sig && sig.async ? wrapAsyncImport(getInstance, fn, sig) : wrapImport(getInstance, fn, sig);
  }};

  for (const name of Object.keys(userImports)) {{
    const sig = sigByName.get(name);
    const module = sig ? sig.module : "env";
    const field = sig ? sig.field : name;
    (importObject[module] ||= {{}})[field] = wrapFor(userImports[name], sig);
  }}

  if (abi) {{
    for (const e of abi.externs) {{
      const bucket = (importObject[e.module] ||= {{}});
      if (bucket[e.field]) continue;
      const resolved = (e.module === "Dream" && builtinDream[e.field])
        ? builtinDream[e.field]
        : resolveGlobal(e.module, e.field);
      bucket[e.field] = resolved
        ? wrapFor(resolved, e)
        : () => {{
            throw new Error(`no JS implementation for extern '${{e.name}}' (${{e.module}}.${{e.field}})`);
          }};
    }}
  }}

  const wasmInstance = await WebAssembly.instantiate(wasmModule, importObject);
  instance = new DreamInstance(wasmInstance);
  return instance;
}}

async function run(source, options = {{}}) {{
  const abi =
    options.abi ?? (typeof source === "string" ? source.replace(/\.wasm$/, ".abi.json") : undefined);
  const mod = await load(source, {{ ...options, abi }});
  mod.run();
  return mod;
}}

export {{ load, run, DreamInstance, TAGS, HEAP_HEADER_SIZE }};
export default {{ load, run, DreamInstance, TAGS, HEAP_HEADER_SIZE }};
"#
    )
}

/// Assemble a selective runtime from live import `(module, field)` pairs.
pub(crate) fn assemble_selective_runtime(live_imports: &[LiveImport]) -> Result<String, Error> {
    let mut chunks: BTreeSet<&str> = BTreeSet::new();
    for (_module, field) in live_imports {
        if let Some(c) = chunk_for_field(field) {
            chunks.insert(c);
        }
    }

    let src = runtime_src_dir();
    let mut out = String::from(
        "// Dream JS interop runtime (selective — generated per compile).\n\
         // Only host chunks required by this module's live imports are included.\n\
         // Full runtime: runtime/dream.js (from runtime/src via scripts/bundle-runtime.mjs).\n",
    );

    for rel in ALWAYS_MODULES {
        let path = src.join(rel);
        let text = fs::read_to_string(&path)?;
        out.push_str(&transform_module(&text, rel));
        out.push('\n');
    }

    // Deterministic chunk order for byte-identical output.
    for chunk in ["js", "http", "fs", "crypto", "gpu", "console_process", "datetime_text", "workers"] {
        if !chunks.contains(chunk) {
            continue;
        }
        let Some(rel) = chunk_file(chunk) else { continue };
        let path = src.join(rel);
        let text = fs::read_to_string(&path)?;
        out.push_str(&transform_module(&text, rel));
        out.push('\n');
    }

    out.push_str(&load_footer(&chunks));
    Ok(out)
}

/// Writes `<wat_stem>.runtime.js` next to the compiled `.wat` / `.wasm`.
pub(crate) fn emit_selective_runtime(
    wat_path: &str,
    live_imports: &[LiveImport],
) -> Result<(), Error> {
    let path = Path::new(wat_path).with_extension("runtime.js");
    let text = assemble_selective_runtime(live_imports)?;
    fs::write(&path, text)?;
    info!("created file: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_print_runtime_omits_gpu_and_fs() {
        let text = assemble_selective_runtime(&[]).expect("assemble");
        assert!(!text.contains("makeGpuHost"), "gpu must be absent:\n{}", &text[..200.min(text.len())]);
        assert!(!text.contains("makeFsHost"));
        assert!(!text.contains("makeCryptoHost"));
        assert!(text.contains("function load("));
        assert!(text.contains("TAGS"));
    }

    #[test]
    fn gpu_field_pulls_gpu_chunk() {
        let live = vec![("Dream".into(), "gpuDispatch".into())];
        let text = assemble_selective_runtime(&live).expect("assemble");
        assert!(text.contains("makeGpuHost"));
        assert!(!text.contains("makeFsHost"));
    }

    #[test]
    fn selective_runtime_is_deterministic() {
        let live = vec![
            ("Dream".into(), "jsGlobal".into()),
            ("Dream".into(), "fileRead".into()),
        ];
        let a = assemble_selective_runtime(&live).unwrap();
        let b = assemble_selective_runtime(&live).unwrap();
        assert_eq!(a, b);
    }
}
