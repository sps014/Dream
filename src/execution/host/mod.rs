//! Wasmtime host glue shared between the CLI runtime ([`super::wasm_runner`]) and the E2E test
//! harness (`tests/e2e_tests.rs`). Both link against the same `env`/`Dream` imports; only the
//! output sink differs (real stdout vs. a captured buffer).
//!
//! The pieces are split by concern so each capability lives next to the stdlib module it backs:
//!   * [`memory`]   - shared string/`char[]` marshaling across the WASM boundary.
//!   * [`file`]     - `src/stdlib/io/file.dream` (synchronous `std::fs`).
//!   * [`regex`]    - `src/stdlib/text/regex.dream` (the `regex` crate).
//!   * [`http`]     - `src/stdlib/net/http_client.dream` (blocking `reqwest` + the async future bridge).
//!   * [`math`]     - the `Math.*` `env` builtins.
//!   * [`console`]  - `src/stdlib/system/system.dream`'s `readLine`/`readKey`/`exit` (the `crossterm` crate).
//!   * [`datetime`] - `src/stdlib/system/datetime.dream`'s wall clock + local timezone offset (the `chrono` crate).

mod console;
mod datetime;
mod file;
mod http;
mod math;
mod memory;
mod regex;

pub use console::{enable_ansi_support, link_console_functions};
pub use datetime::link_datetime_functions;
pub use file::link_file_functions;
pub use http::link_http_functions;
pub use math::link_math_functions;
pub use memory::{read_string_from_memory, write_bytes_to_memory, write_string_to_memory};
pub use regex::link_regex_functions;

#[cfg(test)]
mod contract_tests {
    //! Guards the `@js("Dream", …)` link contract: every native host function registered under the
    //! `Dream` module must be declared by some stdlib prelude bridge, so a rename/removal on one
    //! side can't silently orphan the other. The reverse direction is intentionally *not* checked:
    //! the dynamic `js` interop names (`jsObject`, `jsCallV`, …) are implemented only by the JS host
    //! (`runtime/dream.js`) and trap under wasmtime, so they have no native registration. That
    //! JS-side mirror remains an untested contract maintained by hand.

    use crate::mir::js_abi::HOST_MODULE;
    use std::collections::HashSet;

    /// Source of every host `.rs` that registers `Dream`-module functions (via `func_wrap("Dream",
    /// "name", …)`). `math` binds only `env` builtins, so it is omitted.
    const HOST_SOURCES: &[&str] = &[
        include_str!("console.rs"),
        include_str!("datetime.rs"),
        include_str!("file.rs"),
        include_str!("http.rs"),
        include_str!("regex.rs"),
    ];

    /// Extracts the field name in each `"<HOST_MODULE>", "<name>"` pair, tolerating the line break
    /// `rustfmt` inserts between the two string literals.
    fn names_after_module(src: &str, module: &str) -> Vec<String> {
        let needle = format!("\"{}\"", module);
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(pos) = rest.find(&needle) {
            let after = &rest[pos + needle.len()..];
            // Skip whitespace and the single `,` separating the module from the field literal.
            let trimmed = after.trim_start_matches([' ', '\n', '\r', '\t', ',']);
            if let Some(field) = trimmed.strip_prefix('"') {
                if let Some(end) = field.find('"') {
                    out.push(field[..end].to_string());
                }
            }
            rest = after;
        }
        out
    }

    #[test]
    fn every_native_dream_host_fn_is_declared_in_the_prelude() {
        // Names declared by the prelude via `@js("Dream", "name")`.
        let mut declared: HashSet<String> = HashSet::new();
        for (_, src) in crate::stdlib::PRELUDE_FILES {
            for name in names_after_module(src, HOST_MODULE) {
                declared.insert(name);
            }
        }

        // Names the native host actually registers under the `Dream` module.
        let mut registered: HashSet<String> = HashSet::new();
        for src in HOST_SOURCES {
            for name in names_after_module(src, HOST_MODULE) {
                registered.insert(name);
            }
        }

        assert!(
            !registered.is_empty(),
            "scanner found no native Dream host functions; the pattern likely drifted"
        );

        let orphaned: Vec<&String> = registered.difference(&declared).collect();
        assert!(
            orphaned.is_empty(),
            "native host functions registered under the `Dream` module have no matching \
             `@js(\"Dream\", …)` declaration in the stdlib prelude: {:?}",
            orphaned
        );
    }
}
