//! Dream `@compute` kernel → WGSL emitter.
//!
//! Kernel bodies never enter MIR/WAT. This pass walks the AST of every `@compute` function and
//! emits WGSL text plus binding metadata for the `.abi.json` `"gpu"` section / `.wgsl` sidecar.
//!
//! The emitter tracks nested local scopes so Dream locals shadow buffer/uniform params (WGSL
//! mangling must not rewrite a shadowed name to a storage array), and inserts scalar casts so
//! WGSL never sees mixed `f32`/`i32` arithmetic.

mod context;
mod expr;
mod kernel;
mod output;
mod stmt;
mod ty;
mod types;

pub use output::{gpu_abi_json, join_wgsl_module};
pub use types::{GpuBinding, GpuKernelInfo};

use dream_abi::attributes::has_compute_attr;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::ProgramNode;
use kernel::emit_kernel;

/// Emit WGSL for every `@compute` function in `program`.
/// Unsupported statements are reported on `diagnostics` (and omitted from WGSL).
pub fn collect_compute_kernels(
    program: &ProgramNode<'_>,
    diagnostics: &mut DiagnosticBag,
) -> Vec<GpuKernelInfo> {
    let mut out = Vec::new();
    for func in &program.functions {
        if has_compute_attr(&func.attributes) {
            let saved = diagnostics.file_path.clone();
            if let Some(path) = &func.file_path {
                diagnostics.file_path = Some(path.to_string());
            }
            out.push(emit_kernel(func, diagnostics));
            diagnostics.file_path = saved;
        }
    }
    out
}
