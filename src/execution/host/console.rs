//! Synchronous console host functions (the `Dream` module behind `src/stdlib/system/system.dream`'s
//! `readLine`/`readKey`/`exit`). Browser/Node hosts implement the same names in `runtime/dream.js`.

use std::io::{self, BufRead, Read, Write};
use wasmtime::*;

use crossterm::event::{read, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::memory::write_string_to_memory;

/// Enables ANSI virtual-terminal processing on Windows consoles so `System.setForeground` /
/// `setBackground` escape sequences render as colors instead of literal text. A no-op on other
/// platforms (their terminals already interpret ANSI escapes natively).
pub fn enable_ansi_support() {
    #[cfg(windows)]
    {
        let _ = crossterm::ansi_support::supports_ansi();
    }
}

/// Registers the synchronous console host functions on `linker`. Shared by the CLI runner and the
/// E2E test harness so the native behavior can never drift.
pub fn link_console_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap(
        "Dream",
        "consoleReadLine",
        |mut caller: Caller<'_, ()>| -> Result<i32> {
            let mut line = String::new();
            let stdin = io::stdin();
            let _ = stdin.lock().read_line(&mut line);
            while line.ends_with('\n') || line.ends_with('\r') {
                line.pop();
            }
            write_string_to_memory(&mut caller, &line)
        },
    )?;

    linker.func_wrap("Dream", "consoleReadKey", |_: Caller<'_, ()>| -> i32 {
        let _ = io::stdout().flush();
        if enable_raw_mode().is_err() {
            // Not an interactive terminal (e.g. piped stdin): fall back to reading one byte.
            let mut buf = [0u8; 1];
            return match io::stdin().lock().read_exact(&mut buf) {
                Ok(()) => buf[0] as i32,
                Err(_) => 0,
            };
        }
        let code = loop {
            match read() {
                Ok(Event::Key(key_event)) if key_event.kind == KeyEventKind::Press => {
                    break match key_event.code {
                        KeyCode::Char(c) => c as i32,
                        KeyCode::Enter => 13,
                        KeyCode::Tab => 9,
                        KeyCode::Backspace => 8,
                        KeyCode::Esc => 27,
                        _ => 0,
                    };
                }
                Ok(_) => continue,
                Err(_) => break 0,
            }
        };
        let _ = disable_raw_mode();
        code
    })?;

    linker.func_wrap("Dream", "consoleExit", |code: i32| -> () {
        let _ = io::stdout().flush();
        std::process::exit(code);
    })?;

    Ok(())
}
