//! WebGPU / compute host functions (`system.gpu`). Uses CPU staging buffers on native;
//! WGSL execution is browser-first (`runtime/dream.js`).

use std::collections::HashMap;
use std::sync::Mutex;

use wasmtime::*;

use super::memory::{read_arg_bytes, read_arg_string, write_bytes_to_memory};
use dream_mir::abi;
use dream_mir::async_emit::{F_SLOTS, HOST_POLL_INDEX, KIND_HOST};

struct GpuState {
    next_id: i32,
    buffers: HashMap<i32, Vec<u8>>,
    shaders: HashMap<i32, (String, String)>,
}

impl Default for GpuState {
    fn default() -> Self {
        Self {
            next_id: 1,
            buffers: HashMap::new(),
            shaders: HashMap::new(),
        }
    }
}

fn state() -> &'static Mutex<GpuState> {
    use std::sync::OnceLock;
    static CELL: OnceLock<Mutex<GpuState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(GpuState::default()))
}

fn call_export_2(caller: &mut Caller<'_, ()>, name: &str, a: i32, b: i32) -> Result<()> {
    let func = caller
        .get_export(name)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg(format!("module must export `{name}`")))?
        .typed::<(i32, i32), ()>(&*caller)
        .map_err(|_| Error::msg(format!("unexpected `{name}` signature")))?;
    func.call(&mut *caller, (a, b))?;
    Ok(())
}

fn resolve_host_future_void(caller: &mut Caller<'_, ()>) -> Result<i32> {
    let new_future = caller
        .get_export(abi::EXPORT_NEW_FUTURE)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `__dream_new_future`"))?
        .typed::<(i32, i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `__dream_new_future` signature"))?;
    let future = new_future.call(&mut *caller, (F_SLOTS, HOST_POLL_INDEX, KIND_HOST))?;
    call_export_2(caller, abi::EXPORT_RESOLVE, future, 0)?;
    Ok(future)
}

fn resolve_host_future_bytes(caller: &mut Caller<'_, ()>, bytes: &[u8]) -> Result<i32> {
    let new_future = caller
        .get_export(abi::EXPORT_NEW_FUTURE)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `__dream_new_future`"))?
        .typed::<(i32, i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `__dream_new_future` signature"))?;
    let future = new_future.call(&mut *caller, (F_SLOTS, HOST_POLL_INDEX, KIND_HOST))?;
    let data_ptr = write_bytes_to_memory(caller, bytes)?;
    call_export_2(caller, abi::EXPORT_RESOLVE, future, data_ptr)?;
    Ok(future)
}

/// Link `Dream` gpu* imports used by `system.gpu`.
pub fn link_gpu_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap("Dream", "gpuBufferAllocBytes", |n: i32| -> i32 {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let id = st.next_id;
        st.next_id += 1;
        st.buffers.insert(id, vec![0u8; n.max(0) as usize]);
        id
    })?;

    linker.func_wrap(
        "Dream",
        "gpuBufferWriteBytes",
        |mut caller: Caller<'_, ()>, id: i32, data_ptr: i32| -> Result<()> {
            let bytes = read_arg_bytes(&mut caller, data_ptr)?;
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let buf = st
                .buffers
                .get_mut(&id)
                .ok_or_else(|| Error::msg(format!("unknown GpuBuffer {id}")))?;
            *buf = bytes;
            Ok(())
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuBufferReadBytes",
        |mut caller: Caller<'_, ()>, id: i32, n: i32| -> Result<i32> {
            let bytes = {
                let st = state().lock().unwrap_or_else(|e| e.into_inner());
                let buf = st
                    .buffers
                    .get(&id)
                    .ok_or_else(|| Error::msg(format!("unknown GpuBuffer {id}")))?;
                let take = (n.max(0) as usize).min(buf.len());
                buf[..take].to_vec()
            };
            resolve_host_future_bytes(&mut caller, &bytes)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuDispatch",
        |mut caller: Caller<'_, ()>,
         kernel_ptr: i32,
         _bufs_ptr: i32,
         _ex: i32,
         _ey: i32,
         _ez: i32|
         -> Result<i32> {
            let _name = read_arg_string(&mut caller, kernel_ptr)?;
            // Native: CPU staging only; WGSL runs in the browser host.
            resolve_host_future_void(&mut caller)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuShaderFromWgsl",
        |mut caller: Caller<'_, ()>, source_ptr: i32, entry_ptr: i32| -> Result<i32> {
            let source = read_arg_string(&mut caller, source_ptr)?;
            let entry = read_arg_string(&mut caller, entry_ptr)?;
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let id = st.next_id;
            st.next_id += 1;
            st.shaders.insert(id, (source, entry));
            Ok(id)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuDispatchShader",
        |mut caller: Caller<'_, ()>,
         _shader_id: i32,
         _bufs: i32,
         _wx: i32,
         _wy: i32,
         _wz: i32|
         -> Result<i32> {
            resolve_host_future_void(&mut caller)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuPresentRgba",
        |mut caller: Caller<'_, ()>,
         canvas_id_ptr: i32,
         pixels_ptr: i32,
         _w: i32,
         _h: i32|
         -> Result<i32> {
            let _ = read_arg_string(&mut caller, canvas_id_ptr)?;
            let _ = read_arg_bytes(&mut caller, pixels_ptr)?;
            resolve_host_future_void(&mut caller)
        },
    )?;

    Ok(())
}
