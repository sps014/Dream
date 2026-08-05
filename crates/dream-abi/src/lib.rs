//! Shared compiler ABI constants and registries used by both semantic analysis and MIR.
//!
//! Lives outside `dream-mir` so `dream-sema` can use JS interop names / intrinsics / attributes
//! without depending on the backend.

pub mod attributes;
pub mod intrinsics;
pub mod js_abi;
