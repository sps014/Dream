//! Free-function (and constructor-shaped) call analysis, split by concern:
//! - [`call`]: the main `analyze_function_call` dispatch (indirect/constructor/generic/overload
//!   resolution, arity + default-argument handling) and the shared `substitute_default_args` helper.
//! - [`async_intrinsics`]: the `sleep`/`all`/`any`/`race` async built-ins.
//! - [`generic`]: monomorphizing a generic free function for a call or a first-class function value.

use super::super::*;

mod async_intrinsics;
mod call;
mod generic;
