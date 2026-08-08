//! Copies `DREAM_EMBEDDED_WASM` into `OUT_DIR/embedded.wasm` for `include_bytes!`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = out_dir.join("embedded.wasm");

    println!("cargo:rerun-if-env-changed=DREAM_EMBEDDED_WASM");

    if let Ok(src) = env::var("DREAM_EMBEDDED_WASM") {
        println!("cargo:rerun-if-changed={src}");
        fs::copy(&src, &dest).unwrap_or_else(|e| {
            panic!(
                "dream-runner: failed to copy DREAM_EMBEDDED_WASM ({src}) to {}: {e}",
                dest.display()
            )
        });
    } else {
        // Placeholder so the crate still compiles without an embed (cargo check / default builds).
        // Packed builds always set DREAM_EMBEDDED_WASM.
        if !dest.exists() {
            fs::write(&dest, b"\0asm\x01\x00\x00\x00").expect("write placeholder wasm");
        }
    }
}
