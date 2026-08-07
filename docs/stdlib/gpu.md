# `system.gpu`

WebGPU compute and present from Dream. Auto-imported when any `@compute` kernel is present
(same pattern as `@json` → `system.json`). You can also `import system.gpu;`.

Samples (user-facing code in each README / file):

| Sample | Role |
|--------|------|
| [`saxpy.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/saxpy.dream) | Beginner — one `@compute` + readback |
| [`gpu_ext.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/gpu_ext.dream) | Pass / indirect / `@readonly` |
| [`life/`](https://github.com/sps014/Dream/tree/main/sample/compute/life) | Complex — Gray–Scott reaction–diffusion |
| [`fluid/`](https://github.com/sps014/Dream/tree/main/sample/fluid) | Interactive stable fluids |

Full language tutorial: [Compute shaders](../language/compute.md).

## Device

| API | Role |
|-----|------|
| `Gpu.is_available(): bool` | Sync probe (`navigator.gpu`) |
| `await Gpu.try_init(): Result<bool, GpuError>` | Request adapter/device (idempotent) |
| `Gpu.ready(): bool` | True after successful init |
| `GpuError` | `Error` with `code()` / `message()` |

## Timing

| API | Role |
|-----|------|
| `await Time.delay(ms)` | Wall-clock delay (browser `setTimeout`) |
| `await Time.sleep(ms)` | Cooperative / virtual-clock sleep |
| `await Gpu.frame()` | One display frame (`requestAnimationFrame`) |
| `await Gpu.timestamp(): long` | Queue/host timestamp (ns) |

## Buffers

| API | Role |
|-----|------|
| `GpuBuffer<T>.alloc` / `.from` | Allocate / seed |
| `.length` / `.id` / `.stride` | Element count / host id / byte stride (properties) |
| `write` / `write_at` | Full / partial CPU→GPU upload |
| `read` / `read_at` | Full / partial readback |
| `copy_to(dst, src_offset, dst_offset, count)` | GPU-side element copy (no CPU round-trip) |
| `GpuSwap<T>` | Ping-pong `front` / `back` / `swap` |

`T` must be `unmanaged`. Staging uses `Bytes.of` / `Bytes.to`. `GpuBuffer` is a value `struct` handle.

In `@compute` kernels, storage params are **`GpuBuffer<T>`** (not bare `T[]`); index with `buf[i]` and use `buf.length`. Mark inputs `@readonly` for WGSL `var<storage, read>`:

```dream
@compute(64)
fun scale(@readonly a: GpuBuffer<float>, out: GpuBuffer<float>, n: int): void {
    out[global_id.x] = a[global_id.x] * 2.0;
}
```

```dream
let a = GpuBuffer<float>.from([1.0, 2.0, 3.0]);
let out = GpuBuffer<float>.alloc(3);
let _ = await Gpu.try_init();
let r = await Compute.run_1d("scale", [a, out], 3);
```

## Dispatch

| API | Role |
|-----|------|
| `Compute.run_1d/2d/3d` | Named `@compute` kernels; returns `Result` |
| `Compute.run_2d_uniforms` | Extra uniform blob after extent i32s |
| `Compute.run_resources` | Explicit buffer / texture / sampler id lists |
| `Compute.dispatch_indirect` | `dispatchWorkgroupsIndirect` via `GpuBuffer<int>` (3×u32 workgroup counts) |
| `Uniforms.pack_i32` / `pack_f32` | Build uniform bytes |
| `GpuShader` + `Compute.run_shader` | Raw WGSL escape hatch |
| `ComputePass.begin` / `.dispatch` / `.submit` | Batch several dispatches into one queue submit |
| `GpuDispatchIndirect` | Helper to pack / write indirect workgroup counts |

Pass `GpuBuffer<float>[]` in kernel binding order for `run_*` (same order as `GpuBuffer` params). Use `run_resources` / `ComputePass.dispatch_resources` when the kernel also binds textures or samplers.

## Textures / samplers / present

| API | Role |
|-----|------|
| `GpuTexture.rgba8` | RGBA8 texture |
| `write_rgba` / `write_rgba_at` / `read_rgba` | Upload / GPU→CPU readback |
| `copy_from_buffer` / `copy_to_buffer` | GPU-side buffer↔texture copies |
| `GpuSampler.linear` / `.nearest` | Sampling state for compute |
| `GpuSurface.from_canvas` | Canvas swapchain |
| `configure` / `present` | Resize / present |
| `GpuRenderPass.blit` | Fullscreen blit texture → surface |

Kernel params: `@readonly GpuTexture` → sampled `texture_2d`; plain `GpuTexture` → `texture_storage_2d` (write). Pair sampled textures with a `GpuSampler` param. Builtins: `Gpu.texture_load` / `texture_store` / `texture_sample_level`.

```dream
let surface = GpuSurface.from_canvas("fluid").unwrap_or(/*…*/);
let tex = GpuTexture.rgba8(w, h);
await tex.write_rgba(rgba_bytes);
await GpuRenderPass.blit(surface, tex);
await surface.present();
await Gpu.frame();
```

## Atomics (kernel-only)

| API | Role |
|-----|------|
| `Gpu.atomic_load` / `atomic_store` / `atomic_add` / `atomic_exchange` | WGSL atomics on `GpuBuffer<int>` |

Buffers touched by these helpers are emitted as `array<atomic<i32>>`.

## Native vs browser

| Host | Behavior |
|------|----------|
| Browser (`dream.js`) | Real WebGPU when available |
| Native (`dream run`) | CPU staging; dispatch no-ops WGSL; surface/blit unavailable |

See [Compute shaders](../language/compute.md).
