//! Top-level declaration registration: the analyzer passes that populate the type/symbol tables
//! before bodies are checked, split by declared entity:
//! - [`enums`]: C-style enums and discriminated unions (variant layout, discriminants, generic-union
//!   instantiation) plus generic `extend`-block method registration.
//! - [`structs`]: struct/class registration, value-vs-reference classification, value-containment
//!   soundness, and generic-struct instantiation.
//! - [`globals`]: top-level variable registration and initializer type-checking.
//! - [`functions`]: function-signature registration and the body-analysis / pending-instantiation
//!   fixpoint passes.
//!
//! - [`register_interfaces`]: interface def/method-slot registration, generic-interface
//!   monomorphization, the runtime interface table, and `validate_implements`.
//! - [`register_methods`]: struct-method and `extend`-block method registration plus object-protocol
//!   override validation.
//!
//! All are `impl Analyzer` blocks split to keep each focused.

use super::*;

mod enums;
mod functions;
mod globals;
mod register_interfaces;
mod register_methods;
mod structs;
