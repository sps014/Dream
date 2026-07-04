use super::host::{
    enable_ansi_support, link_console_functions, link_datetime_functions, link_file_functions,
    link_http_functions, link_math_functions, link_regex_functions, read_string_from_memory,
};
use std::fs;
use wasmtime::*;

pub fn execute_wasm(wat_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    enable_ansi_support();
    let wat_content = fs::read_to_string(wat_path)?;
    let wasm_bytes = wat::parse_str(&wat_content)?;

    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes)?;

    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);

    linker.func_wrap("env", "print_int", |v: i32| {
        print!("{}", v);
    })?;

    linker.func_wrap("env", "print_float", |v: f32| {
        print!("{}", v);
    })?;

    linker.func_wrap("env", "print_double", |v: f64| {
        print!("{}", v);
    })?;

    linker.func_wrap("env", "print_char", |v: i32| {
        if let Some(c) = char::from_u32(v as u32) {
            print!("{}", c);
        }
    })?;

    linker.func_wrap(
        "env",
        "print_string",
        |mut caller: Caller<'_, ()>, ptr: i32| -> Result<()> {
            let memory = caller
                .get_export("memory")
                .and_then(Extern::into_memory)
                .ok_or_else(|| Error::msg("module must export `memory`"))?;
            let s = read_string_from_memory(&memory, &caller, ptr);
            print!("{}", s);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "env",
        "println",
        |mut caller: Caller<'_, ()>, ptr: i32| -> Result<()> {
            let memory = caller
                .get_export("memory")
                .and_then(Extern::into_memory)
                .ok_or_else(|| Error::msg("module must export `memory`"))?;
            let s = read_string_from_memory(&memory, &caller, ptr);
            println!("{}", s);
            Ok(())
        },
    )?;

    link_math_functions(&mut linker)?;
    link_file_functions(&mut linker)?;
    link_http_functions(&mut linker)?;
    link_regex_functions(&mut linker)?;
    link_console_functions(&mut linker)?;
    link_datetime_functions(&mut linker)?;
    linker.func_wrap("env", "strlen", |_: i32| -> i32 { 0 })?;
    linker.func_wrap("env", "debug_get_free_list_head", || -> i32 { 0 })?;

    // JS-interop externs (e.g. the `Dream` host module behind the dynamic `js` type/regex/fetch, or any
    // user `@js(...)` import) have no native implementation. Stub every still-unresolved import
    // as a trap so modules that merely *declare* them still instantiate and run under wasmtime;
    // calling one without a JS host traps, matching `runtime/dream.js`'s thrower stubs.
    linker.define_unknown_imports_as_traps(&module)?;

    let instance = linker.instantiate(&mut store, &module)?;

    if let Ok(main_func) = instance.get_typed_func::<(), ()>(&mut store, crate::mir::abi::ENTRY_FN)
    {
        main_func.call(&mut store, ())?;
    } else {
        println!("No main function found in module");
    }

    Ok(())
}
