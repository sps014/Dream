//! Synchronous filesystem host functions (the `Dream` module behind `system.io.File`),
//! implemented over `std::fs`. Browser/Node hosts implement the same names in `runtime/dream.js`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;
use wasmtime::*;

use super::memory::{
    read_arg_bytes, read_arg_string, write_bytes_to_memory, write_string_to_memory,
};

/// Open error codes returned by `fileOpen` (negative `i32` values).
const OPEN_ENOENT: i32 = -1;
const OPEN_EACCES: i32 = -2;
const OPEN_EINVAL: i32 = -3;
const OPEN_EIO: i32 = -4;

struct HandleEntry {
    file: File,
}

fn handles() -> &'static Mutex<IndexMap<u32, HandleEntry>> {
    static HANDLES: OnceLock<Mutex<IndexMap<u32, HandleEntry>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(IndexMap::new()))
}

static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

fn map_open_mode(mode: &str) -> Option<OpenOptions> {
    let mut opts = OpenOptions::new();
    match mode {
        "r" => {
            opts.read(true);
        }
        "w" => {
            opts.write(true).create(true).truncate(true);
        }
        "a" => {
            opts.append(true).create(true);
        }
        "r+" => {
            opts.read(true).write(true);
        }
        "w+" => {
            opts.read(true).write(true).create(true).truncate(true);
        }
        "a+" => {
            opts.read(true).append(true).create(true);
        }
        _ => return None,
    }
    Some(opts)
}

fn map_open_error(e: std::io::Error) -> i32 {
    match e.kind() {
        std::io::ErrorKind::NotFound => OPEN_ENOENT,
        std::io::ErrorKind::PermissionDenied => OPEN_EACCES,
        _ => OPEN_EIO,
    }
}

fn take_handle(id: u32) -> Option<File> {
    let mut table = handles().lock().unwrap_or_else(|e| e.into_inner());
    table.swap_remove(&id).map(|e| e.file)
}

fn with_handle<F, R>(id: i32, f: F) -> Option<R>
where
    F: FnOnce(&mut File) -> R,
{
    if id <= 0 {
        return None;
    }
    let mut table = handles().lock().unwrap_or_else(|e| e.into_inner());
    let entry = table.get_mut(&(id as u32))?;
    Some(f(&mut entry.file))
}

/// Registers the synchronous filesystem host functions on `linker`. Shared by the CLI runner and
/// the E2E test harness so the native behavior can never drift.
pub fn link_file_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap(
        "Dream",
        "fileRead",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            // The bridge ABI returns a bare string pointer with no error channel (the Dream `File.read`
            // wrapper guards with `exists()` and reports `Err` itself), so a genuine read failure here
            // can only surface as empty content. Log it rather than swallowing it silently, and read
            // as bytes + lossy-decode so a non-UTF-8 file still yields its content instead of "".
            let content = match fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "fileRead failed; returning empty string");
                    String::new()
                }
            };
            write_string_to_memory(&mut caller, &content)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileWrite",
        |mut caller: Caller<'_, ()>, path_ptr: i32, content_ptr: i32| -> Result<i64> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            let content = read_arg_string(&mut caller, content_ptr)?;
            Ok(match fs::write(&path, content.as_bytes()) {
                Ok(()) => content.len() as i64,
                Err(_) => -1,
            })
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileAppend",
        |mut caller: Caller<'_, ()>, path_ptr: i32, content_ptr: i32| -> Result<i64> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            let content = read_arg_string(&mut caller, content_ptr)?;
            let result = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(content.as_bytes()));
            Ok(match result {
                Ok(()) => content.len() as i64,
                Err(_) => -1,
            })
        },
    )?;

    // Binary I/O: a single bulk copy between the file and a Dream `byte[]`, no string round-trip.
    linker.func_wrap(
        "Dream",
        "fileReadBytes",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            // As with `fileRead`, the ABI has no error channel; log failures instead of masking them.
            let bytes = fs::read(&path).unwrap_or_else(|e| {
                tracing::warn!(path = %path, error = %e, "fileReadBytes failed; returning empty array");
                Vec::new()
            });
            write_bytes_to_memory(&mut caller, &bytes)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileWriteBytes",
        |mut caller: Caller<'_, ()>, path_ptr: i32, data_ptr: i32| -> Result<i64> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            let bytes = read_arg_bytes(&mut caller, data_ptr)?;
            Ok(match fs::write(&path, &bytes) {
                Ok(()) => bytes.len() as i64,
                Err(_) => -1,
            })
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileExists",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(Path::new(&path).exists() as i32)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileDelete",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(fs::remove_file(&path).is_ok() as i32)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileSize",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i64> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(-1))
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileIsDir",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(Path::new(&path).is_dir() as i32)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "dirList",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            let joined = match fs::read_dir(&path) {
                Ok(entries) => {
                    let mut names: Vec<String> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect();
                    names.sort();
                    names.join("\n")
                }
                Err(_) => String::new(),
            };
            write_string_to_memory(&mut caller, &joined)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "dirCreate",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(fs::create_dir(&path).is_ok() as i32)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "dirCreateAll",
        |mut caller: Caller<'_, ()>, path_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            Ok(fs::create_dir_all(&path).is_ok() as i32)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileOpen",
        |mut caller: Caller<'_, ()>, path_ptr: i32, mode_ptr: i32| -> Result<i32> {
            let path = read_arg_string(&mut caller, path_ptr)?;
            let mode = read_arg_string(&mut caller, mode_ptr)?;
            let Some(opts) = map_open_mode(&mode) else {
                return Ok(OPEN_EINVAL);
            };
            match opts.open(&path) {
                Ok(file) => {
                    let id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
                    let mut table = handles().lock().unwrap_or_else(|e| e.into_inner());
                    table.insert(
                        id,
                        HandleEntry {
                            file,
                        },
                    );
                    Ok(id as i32)
                }
                Err(e) => Ok(map_open_error(e)),
            }
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileHandleRead",
        |mut caller: Caller<'_, ()>, id: i32, n: i32| -> Result<i32> {
            let count = n.max(0) as usize;
            let mut buf = vec![0u8; count];
            let read_len = with_handle(id, |file| {
                file.read(&mut buf).unwrap_or(0)
            })
                .unwrap_or(0);
            write_bytes_to_memory(&mut caller, &buf[..read_len])
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileHandleWrite",
        |mut caller: Caller<'_, ()>, id: i32, data_ptr: i32| -> Result<i64> {
            let bytes = read_arg_bytes(&mut caller, data_ptr)?;
            let written = with_handle(id, |file| file.write(&bytes).unwrap_or(0))
                .unwrap_or(0);
            if written == 0 && !bytes.is_empty() {
                Ok(-1)
            } else {
                Ok(written as i64)
            }
        },
    )?;

    linker.func_wrap(
        "Dream",
        "fileHandleSeek",
        |_: Caller<'_, ()>, id: i32, pos: i64| -> Result<i32> {
            let ok = with_handle(id, |file| {
                file.seek(SeekFrom::Start(pos as u64)).is_ok()
            })
                .unwrap_or(false);
            Ok(if ok { 0 } else { -1 })
        },
    )?;

    linker.func_wrap("Dream", "fileHandleClose", |_: Caller<'_, ()>, id: i32| -> Result<()> {
        take_handle(id as u32);
        Ok(())
    })?;

    Ok(())
}
