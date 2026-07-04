//! Native execution of compiled Dream modules under wasmtime. [`wasm_runner`] instantiates a `.wat`
//! module, wires the `env` print imports and the [`host`] function modules (file/http/regex/console/
//! datetime/math), and runs its entry point; [`host`] holds those host functions and the linear-
//! memory marshaling they share. The browser/Node hosts mirror the same function names in
//! `runtime/dream.js`, so a program behaves identically across runtimes.

pub mod debugger;
pub mod host;
pub mod wasm_runner;
