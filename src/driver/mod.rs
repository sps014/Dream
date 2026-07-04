//! Compilation driver: wires the phases into an end-to-end build. [`compiler`] is the entry point
//! that runs lex -> parse -> analyze -> lower -> optimize -> emit and writes the module; the rest
//! are the cross-cutting pieces it orchestrates: [`source_loader`]/[`prelude`] (assembling user
//! sources with the bundled stdlib), [`interface_defaults`] (synthesizing inherited default-method
//! bodies before analysis), [`json_derive`] (`@json` (de)serialization codegen), [`abi`] (shared
//! runtime layout constants), and [`error`] (the driver-level error type).

pub mod abi;
pub mod compiler;
pub mod error;
pub mod interface_defaults;
pub mod json_derive;
pub mod prelude;
pub mod source_loader;
