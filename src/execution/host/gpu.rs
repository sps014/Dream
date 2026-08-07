//! WebGPU / compute host functions (`system.gpu`). Uses CPU staging buffers on native;
//! WGSL execution and canvas present are browser-first (`runtime/dream.js`).

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
    textures: HashMap<i32, (i32, i32, Vec<u8>)>, // w, h, rgba
    samplers: HashMap<i32, i32>,                  // filter mode
    passes: HashMap<i32, Vec<()>>,                // staging: empty ops list
    ready: bool,
}

impl Default for GpuState {
    fn default() -> Self {
        Self {
            next_id: 1,
            buffers: HashMap::new(),
            shaders: HashMap::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            passes: HashMap::new(),
            ready: false,
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

fn resolve_host_future_i32(caller: &mut Caller<'_, ()>, value: i32) -> Result<i32> {
    let new_future = caller
        .get_export(abi::EXPORT_NEW_FUTURE)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `__dream_new_future`"))?
        .typed::<(i32, i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `__dream_new_future` signature"))?;
    let future = new_future.call(&mut *caller, (F_SLOTS, HOST_POLL_INDEX, KIND_HOST))?;
    call_export_2(caller, abi::EXPORT_RESOLVE, future, value)?;
    Ok(future)
}

fn resolve_host_future_void(caller: &mut Caller<'_, ()>) -> Result<i32> {
    resolve_host_future_i32(caller, 0)
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

fn resolve_host_future_long(caller: &mut Caller<'_, ()>, value: i64) -> Result<i32> {
    // Store i64 in a tiny heap block and resolve pointer — Dream long results from async
    // host typically use the raw i64 via resolve. Mirror void: put value in F_RESULT by
    // resolving with truncated i32 is wrong. Native timestamp returns via sync path instead.
    let _ = value;
    resolve_host_future_i32(caller, 0)
}

/// Link `Dream` gpu* imports used by `system.gpu`.
pub fn link_gpu_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap("Dream", "gpuIsAvailable", || -> i32 { 0 })?;
    linker.func_wrap("Dream", "gpuReady", || -> i32 {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        i32::from(st.ready)
    })?;
    linker.func_wrap("Dream", "gpuTryInit", |mut caller: Caller<'_, ()>| -> Result<i32> {
        {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            st.ready = true;
        }
        resolve_host_future_i32(&mut caller, 0)
    })?;
    linker.func_wrap("Dream", "gpuFrame", |mut caller: Caller<'_, ()>| -> Result<i32> {
        resolve_host_future_void(&mut caller)
    })?;
    linker.func_wrap(
        "Dream",
        "gpuTimestamp",
        |mut caller: Caller<'_, ()>| -> Result<i32> {
            // Staging: resolve 0; Dream reads long via host marshal for BigInt on JS.
            resolve_host_future_long(&mut caller, 0)
        },
    )?;

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
        "gpuBufferWriteBytesAt",
        |mut caller: Caller<'_, ()>, id: i32, byte_offset: i32, data_ptr: i32| -> Result<()> {
            let bytes = read_arg_bytes(&mut caller, data_ptr)?;
            let off = byte_offset.max(0) as usize;
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let buf = st
                .buffers
                .get_mut(&id)
                .ok_or_else(|| Error::msg(format!("unknown GpuBuffer {id}")))?;
            let end = off + bytes.len();
            if end > buf.len() {
                buf.resize(end, 0);
            }
            buf[off..end].copy_from_slice(&bytes);
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
        "gpuBufferReadBytesAt",
        |mut caller: Caller<'_, ()>, id: i32, byte_offset: i32, n: i32| -> Result<i32> {
            let bytes = {
                let st = state().lock().unwrap_or_else(|e| e.into_inner());
                let buf = st
                    .buffers
                    .get(&id)
                    .ok_or_else(|| Error::msg(format!("unknown GpuBuffer {id}")))?;
                let off = byte_offset.max(0) as usize;
                let take = n.max(0) as usize;
                if off >= buf.len() {
                    vec![0u8; take]
                } else {
                    let end = (off + take).min(buf.len());
                    let mut out = buf[off..end].to_vec();
                    out.resize(take, 0);
                    out
                }
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
         _tex_ptr: i32,
         _samp_ptr: i32,
         _ex: i32,
         _ey: i32,
         _ez: i32,
         uniforms_ptr: i32|
         -> Result<i32> {
            let _name = read_arg_string(&mut caller, kernel_ptr)?;
            let _ = read_arg_bytes(&mut caller, uniforms_ptr)?;
            resolve_host_future_i32(&mut caller, 0)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuDispatchIndirect",
        |mut caller: Caller<'_, ()>,
         kernel_ptr: i32,
         _bufs: i32,
         _tex: i32,
         _samp: i32,
         _indirect: i32,
         _off: i32|
         -> Result<i32> {
            let _ = read_arg_string(&mut caller, kernel_ptr)?;
            resolve_host_future_i32(&mut caller, 0)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuBufferCopy",
        |src_id: i32, dst_id: i32, src_off: i32, dst_off: i32, size: i32| {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let n = size.max(0) as usize;
            let so = src_off.max(0) as usize;
            let d_off = dst_off.max(0) as usize;
            let src = st
                .buffers
                .get(&src_id)
                .cloned()
                .unwrap_or_default();
            let dst = st.buffers.entry(dst_id).or_default();
            let end = d_off + n;
            if end > dst.len() {
                dst.resize(end, 0);
            }
            let take = n.min(src.len().saturating_sub(so));
            if take > 0 {
                dst[d_off..d_off + take].copy_from_slice(&src[so..so + take]);
            }
        },
    )?;

    linker.func_wrap("Dream", "gpuSamplerCreate", |filter: i32| -> i32 {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let id = st.next_id;
        st.next_id += 1;
        st.samplers.insert(id, filter);
        id
    })?;

    linker.func_wrap("Dream", "gpuPassBegin", || -> i32 {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let id = st.next_id;
        st.next_id += 1;
        st.passes.insert(id, Vec::new());
        id
    })?;

    linker.func_wrap(
        "Dream",
        "gpuPassDispatch",
        |mut caller: Caller<'_, ()>,
         _pass: i32,
         kernel_ptr: i32,
         _bufs: i32,
         _tex: i32,
         _samp: i32,
         _ex: i32,
         _ey: i32,
         _ez: i32,
         uniforms_ptr: i32| {
            let _ = read_arg_string(&mut caller, kernel_ptr);
            let _ = read_arg_bytes(&mut caller, uniforms_ptr);
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuPassDispatchIndirect",
        |mut caller: Caller<'_, ()>,
         _pass: i32,
         kernel_ptr: i32,
         _bufs: i32,
         _tex: i32,
         _samp: i32,
         _indirect: i32,
         _off: i32| {
            let _ = read_arg_string(&mut caller, kernel_ptr);
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuPassSubmit",
        |mut caller: Caller<'_, ()>, pass_id: i32| -> Result<i32> {
            {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                st.passes.remove(&pass_id);
            }
            resolve_host_future_i32(&mut caller, 0)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuTextureCopyFromBuffer",
        |tex_id: i32, buf_id: i32, byte_offset: i32, x: i32, y: i32, w: i32, h: i32| {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let Some(buf) = st.buffers.get(&buf_id).cloned() else {
                return;
            };
            let Some((tw, _th, tex)) = st.textures.get_mut(&tex_id) else {
                return;
            };
            let off = byte_offset.max(0) as usize;
            let px = x.max(0) as usize;
            let py = y.max(0) as usize;
            let pw = w.max(0) as usize;
            let ph = h.max(0) as usize;
            for row in 0..ph {
                let dst = ((py + row) * (*tw as usize) + px) * 4;
                let src = off + row * pw * 4;
                if src + pw * 4 <= buf.len() && dst + pw * 4 <= tex.len() {
                    tex[dst..dst + pw * 4].copy_from_slice(&buf[src..src + pw * 4]);
                }
            }
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuTextureCopyToBuffer",
        |tex_id: i32, buf_id: i32, byte_offset: i32, x: i32, y: i32, w: i32, h: i32| {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let Some((tw, _th, tex)) = st.textures.get(&tex_id).cloned() else {
                return;
            };
            let Some(buf) = st.buffers.get_mut(&buf_id) else {
                return;
            };
            let off = byte_offset.max(0) as usize;
            let px = x.max(0) as usize;
            let py = y.max(0) as usize;
            let pw = w.max(0) as usize;
            let ph = h.max(0) as usize;
            let need = off + pw * ph * 4;
            if need > buf.len() {
                buf.resize(need, 0);
            }
            for row in 0..ph {
                let src = ((py + row) * (tw as usize) + px) * 4;
                let dst = off + row * pw * 4;
                if src + pw * 4 <= tex.len() && dst + pw * 4 <= buf.len() {
                    buf[dst..dst + pw * 4].copy_from_slice(&tex[src..src + pw * 4]);
                }
            }
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
            resolve_host_future_i32(&mut caller, 0)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuTextureCreateRgba8",
        |w: i32, h: i32| -> i32 {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let id = st.next_id;
            st.next_id += 1;
            let ww = w.max(1);
            let hh = h.max(1);
            st.textures
                .insert(id, (ww, hh, vec![0u8; (ww * hh * 4) as usize]));
            id
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuTextureWriteRgba",
        |mut caller: Caller<'_, ()>,
         id: i32,
         pixels_ptr: i32,
         x: i32,
         y: i32,
         w: i32,
         h: i32|
         -> Result<i32> {
            let pixels = read_arg_bytes(&mut caller, pixels_ptr)?;
            {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                let tex = st
                    .textures
                    .get_mut(&id)
                    .ok_or_else(|| Error::msg(format!("unknown GpuTexture {id}")))?;
                let (tw, _th, buf) = tex;
                let px = x.max(0) as usize;
                let py = y.max(0) as usize;
                let pw = w.max(0) as usize;
                let ph = h.max(0) as usize;
                for row in 0..ph {
                    let dst = ((py + row) * (*tw as usize) + px) * 4;
                    let src = row * pw * 4;
                    if src + pw * 4 <= pixels.len() && dst + pw * 4 <= buf.len() {
                        buf[dst..dst + pw * 4].copy_from_slice(&pixels[src..src + pw * 4]);
                    }
                }
            }
            resolve_host_future_i32(&mut caller, 0)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "gpuTextureReadRgba",
        |mut caller: Caller<'_, ()>, id: i32| -> Result<i32> {
            let bytes = {
                let st = state().lock().unwrap_or_else(|e| e.into_inner());
                let tex = st
                    .textures
                    .get(&id)
                    .ok_or_else(|| Error::msg(format!("unknown GpuTexture {id}")))?;
                tex.2.clone()
            };
            resolve_host_future_bytes(&mut caller, &bytes)
        },
    )?;

    linker.func_wrap("Dream", "gpuSurfaceFromCanvas", |_id_ptr: i32| -> i32 { -1 })?;
    linker.func_wrap(
        "Dream",
        "gpuSurfaceConfigure",
        |_id: i32, _w: i32, _h: i32| {},
    )?;
    linker.func_wrap(
        "Dream",
        "gpuSurfacePresent",
        |mut caller: Caller<'_, ()>, _id: i32| -> Result<i32> {
            resolve_host_future_i32(&mut caller, 1) // UNAVAILABLE
        },
    )?;
    linker.func_wrap(
        "Dream",
        "gpuRenderBlit",
        |mut caller: Caller<'_, ()>, _sid: i32, _tid: i32| -> Result<i32> {
            resolve_host_future_i32(&mut caller, 1)
        },
    )?;

    // delayMs for Time.delay
    linker.func_wrap(
        "Dream",
        "delayMs",
        |mut caller: Caller<'_, ()>, _ms: i32| -> Result<i32> {
            resolve_host_future_void(&mut caller)
        },
    )?;

    Ok(())
}
