//! Public GPU kernel metadata types.

/// One storage/uniform/texture/sampler binding derived from a kernel parameter.
#[derive(Debug, Clone)]
pub struct GpuBinding {
    pub name: String,
    pub binding: u32,
    /// `"storage"`, `"uniform"`, `"texture"`, `"storage_texture"`, or `"sampler"`.
    pub kind: &'static str,
    /// WGSL element / scalar / texture type (`f32`, `i32`, `texture_2d<f32>`, …).
    pub wgsl_ty: String,
    pub read_write: bool,
    /// When true, storage element type is `atomic<…>` (int/uint buffers used with atomics).
    pub atomic: bool,
}

/// Metadata for one `@compute` kernel.
#[derive(Debug, Clone)]
pub struct GpuKernelInfo {
    pub name: String,
    pub entry: String,
    pub workgroup: (u32, u32, u32),
    pub bindings: Vec<GpuBinding>,
    pub wgsl: String,
}
