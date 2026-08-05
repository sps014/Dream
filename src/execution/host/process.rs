//! Process / platform host functions (`Dream` module behind `System.platform` / `env` / `args` /
//! `cwd`). Browser/Node hosts implement the same names in `runtime/dream.js`.

use std::env;
use std::path::PathBuf;
use wasmtime::*;

use super::memory::{read_arg_string, write_string_to_memory};

/// Registers process/platform host functions on `linker`.
pub fn link_process_functions(linker: &mut Linker<()>) -> Result<()> {
    // 0 = Native, 1 = Node, 2 = Browser, 3 = Unknown
    linker.func_wrap("Dream", "processPlatform", || -> i32 { 0 })?;

    // 0 = Unix, 1 = Windows, 2 = Unknown
    linker.func_wrap("Dream", "processOsFamily", || -> i32 {
        if cfg!(windows) {
            1
        } else {
            0
        }
    })?;

    linker.func_wrap(
        "Dream",
        "processArgs",
        |mut caller: Caller<'_, ()>| -> Result<i32> {
            // Skip argv[0] (exe); join remaining args with '\n' (same wire as dirList).
            let joined = env::args().skip(1).collect::<Vec<_>>().join("\n");
            write_string_to_memory(&mut caller, &joined)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "processExePath",
        |mut caller: Caller<'_, ()>| -> Result<i32> {
            let path = env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            write_string_to_memory(&mut caller, &path)
        },
    )?;

    // Tagged: "1" + value when set; "" when unset.
    linker.func_wrap(
        "Dream",
        "processEnvGet",
        |mut caller: Caller<'_, ()>, name_ptr: i32| -> Result<i32> {
            let name = read_arg_string(&mut caller, name_ptr)?;
            let encoded = match env::var(&name) {
                Ok(v) => format!("1{v}"),
                Err(_) => String::new(),
            };
            write_string_to_memory(&mut caller, &encoded)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "processEnvSet",
        |mut caller: Caller<'_, ()>, name_ptr: i32, value_ptr: i32| -> Result<()> {
            let name = read_arg_string(&mut caller, name_ptr)?;
            let value = read_arg_string(&mut caller, value_ptr)?;
            env::set_var(name, value);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "Dream",
        "processCwd",
        |mut caller: Caller<'_, ()>| -> Result<i32> {
            let cwd = env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .display()
                .to_string();
            write_string_to_memory(&mut caller, &cwd)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "processSetCwd",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(env::set_current_dir(path).is_ok() as i32)
        },
    )?;

    Ok(())
}
