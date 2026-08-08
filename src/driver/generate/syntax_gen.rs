//! Generic syntax-DSL expand: run a Dream `harness.dream` beside each registered
//! `@syntax_block` generator. No markup/DSL logic lives here — only snapshot → WASM → replace.

use super::context::GeneratorContext;
use super::registration::RegisteredGenerator;
use super::syntax::SyntaxNodeId;
use dream_diagnostics::DiagnosticBag;
use std::collections::HashSet;
use std::path::Path;
#[cfg(feature = "native")]
use std::path::PathBuf;

#[cfg(feature = "native")]
use std::io::Write;
#[cfg(feature = "native")]
use std::sync::Mutex;

#[cfg(feature = "native")]
const SNAPSHOT_ENV: &str = "DREAM_SYNTAX_GEN_SNAPSHOT";
#[cfg(feature = "native")]
const OK_MARKER: &str = "__DREAM_GEN_OK__";
#[cfg(feature = "native")]
const ERR_MARKER: &str = "__DREAM_GEN_ERR__";

/// For every registered generator that claims syntax blocks and ships a sibling `harness.dream`,
/// expand matching sites via that Dream harness.
pub fn expand_syntax_blocks(ctx: &mut GeneratorContext, diagnostics: &mut DiagnosticBag) {
    let gens: Vec<RegisteredGenerator> = ctx
        .registered
        .iter()
        .filter(|g| !g.syntax_blocks.is_empty())
        .cloned()
        .collect();
    if gens.is_empty() {
        return;
    }

    for gen in gens {
        let mut site_ids: Vec<SyntaxNodeId> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for name in &gen.syntax_blocks {
            for id in ctx.syntax_blocks(name) {
                if seen.insert(id.0) {
                    site_ids.push(id);
                }
            }
        }
        if site_ids.is_empty() {
            continue;
        }

        let gen_path = Path::new(&gen.file_path);
        let Some(dir) = gen_path.parent() else {
            diagnostics.report_error(
                format!(
                    "syntax generator '{}': cannot resolve directory for '{}'",
                    gen.name, gen.file_path
                ),
                None,
            );
            continue;
        };
        let harness_path = dir.join("harness.dream");
        if !harness_path.is_file() {
            diagnostics.report_error(
                format!(
                    "syntax generator '{}': expected harness.dream next to '{}'",
                    gen.name, gen.file_path
                ),
                None,
            );
            continue;
        }

        #[cfg(not(feature = "native"))]
        {
            let _ = (&site_ids, &harness_path);
            diagnostics.report_error(
                "syntax-DSL expand requires the native compiler feature (wasmtime host)"
                    .to_string(),
                None,
            );
            return;
        }

        #[cfg(feature = "native")]
        {
            let snapshot = build_snapshot(ctx, &site_ids);
            match run_harness(&harness_path, &snapshot) {
                Ok(replacements) => {
                    for (id, source) in replacements {
                        ctx.replace(id, source);
                    }
                }
                Err(err) => match err {
                    HarnessError::Site { id, message } => {
                        ctx.error(id, message);
                    }
                    HarnessError::General(message) => {
                        diagnostics.report_error(message, None);
                    }
                },
            }
        }
    }
}

#[cfg(feature = "native")]
fn build_snapshot(ctx: &GeneratorContext, site_ids: &[SyntaxNodeId]) -> String {
    let mut blocks = String::from("[");
    let mut first = true;
    for id in site_ids {
        let Some(site) = ctx.syntax.block_keys.get(id) else {
            continue;
        };
        if !first {
            blocks.push(',');
        }
        first = false;
        blocks.push_str("{\"id\":");
        blocks.push_str(&id.0.to_string());
        blocks.push_str(",\"name\":");
        blocks.push_str(&json_escape(&site.name));
        blocks.push_str(",\"body\":");
        blocks.push_str(&json_escape(&site.body_text));
        blocks.push_str(",\"splices\":");
        blocks.push_str(&json_string_array(&site.splice_sources));
        blocks.push('}');
    }
    blocks.push(']');
    format!("{{\"blocks\":{blocks}}}")
}

#[cfg(feature = "native")]
fn json_string_array(items: &[String]) -> String {
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
enum HarnessError {
    Site { id: SyntaxNodeId, message: String },
    General(String),
}

#[cfg(feature = "native")]
fn run_harness(
    harness_path: &Path,
    snapshot: &str,
) -> Result<Vec<(SyntaxNodeId, String)>, HarnessError> {
    static SNAPSHOT_GUARD: Mutex<()> = Mutex::new(());
    let _guard = SNAPSHOT_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let wat_path = harness_wat_path(harness_path).map_err(HarnessError::General)?;
    let mut snap_file = snap_tempfile().map_err(HarnessError::General)?;
    snap_file
        .write_all(snapshot.as_bytes())
        .map_err(|e| HarnessError::General(format!("syntax generator: write snapshot: {e}")))?;
    let snap_path = snap_file.path.to_string_lossy().into_owned();

    std::env::set_var(SNAPSHOT_ENV, &snap_path);
    let output = crate::execution::wasm_runner::execute_wasm_capturing(&wat_path).map_err(|e| {
        HarnessError::General(format!("syntax generator: failed to run harness: {e}"))
    })?;
    std::env::remove_var(SNAPSHOT_ENV);
    drop(snap_file);

    parse_harness_output(&output)
}

#[cfg(feature = "native")]
fn parse_harness_output(output: &str) -> Result<Vec<(SyntaxNodeId, String)>, HarnessError> {
    let trimmed = output.trim_start();
    if let Some(rest) = trimmed.strip_prefix(OK_MARKER) {
        let body = rest.trim_start_matches('\n');
        let mut out = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((id_str, expr)) = line.split_once('\t') else {
                return Err(HarnessError::General(format!(
                    "syntax generator: bad OK line (expected id\\texpr): {line}"
                )));
            };
            let id: u32 = id_str.parse().map_err(|_| {
                HarnessError::General(format!("syntax generator: bad node id '{id_str}'"))
            })?;
            out.push((SyntaxNodeId(id), expr.to_string()));
        }
        return Ok(out);
    }
    if let Some(rest) = trimmed.strip_prefix(ERR_MARKER) {
        let body = rest.trim_start_matches('\n').trim();
        let first = body.lines().next().unwrap_or(body);
        if let Some((id_str, message)) = first.split_once('\t') {
            if let Ok(id) = id_str.parse::<u32>() {
                return Err(HarnessError::Site {
                    id: SyntaxNodeId(id),
                    message: message.to_string(),
                });
            }
        }
        let message = if first.is_empty() {
            "syntax generator harness failed".to_string()
        } else {
            first.to_string()
        };
        return Err(HarnessError::General(message));
    }
    Err(HarnessError::General(format!(
        "syntax generator: unexpected harness output: {output}"
    )))
}

#[cfg(feature = "native")]
fn harness_wat_path(harness_path: &Path) -> Result<String, String> {
    let fingerprint = harness_fingerprint(harness_path)?;
    let entry = super::current_entry_file();
    let dir = super::manifest::harness_cache_dir(entry.as_deref(), "syntax-gen", fingerprint);
    std::fs::create_dir_all(&dir).map_err(|e| format!("syntax generator: create cache dir: {e}"))?;
    let wat_path = dir.join("harness.wat");
    if wat_path.is_file() {
        return Ok(wat_path.to_string_lossy().into_owned());
    }
    let src = harness_path
        .to_str()
        .ok_or_else(|| "syntax generator: non-UTF-8 harness path".to_string())?
        .to_string();
    let out = wat_path.to_string_lossy().into_owned();
    let compiler =
        crate::driver::compiler::Compiler::new(crate::driver::compiler::Target::Wasm)
            .with_skip_generators(true)
            .with_release(true);
    compiler
        .compile(&src, &out)
        .map_err(|e| format!("syntax generator: failed to compile harness: {e:?}"))?;
    Ok(out)
}

#[cfg(feature = "native")]
fn harness_fingerprint(harness_path: &Path) -> Result<u64, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    harness_path.hash(&mut h);
    let dir = harness_path
        .parent()
        .ok_or_else(|| "syntax generator: harness has no parent dir".to_string())?;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("syntax generator: read harness dir: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("dream"))
        .collect();
    paths.sort();
    for p in paths {
        let bytes = std::fs::read(&p).map_err(|e| format!("syntax generator: read {p:?}: {e}"))?;
        p.hash(&mut h);
        bytes.hash(&mut h);
    }
    Ok(h.finish())
}

#[cfg(feature = "native")]
struct SnapTempFile {
    path: PathBuf,
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
        "dream-syntax-gen-snap-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("syntax generator: create snapshot file: {e}"))?;
    Ok(SnapTempFile { path, file })
}
