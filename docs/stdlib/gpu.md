# `system.gpu`

WebGPU compute and present from Dream. Auto-imported when any `@compute` kernel is present
(same pattern as `@json` → `system.json`). You can also `import system.gpu;`.

Use this package when you need GPU-parallel work (simulations, image processing, particle systems) or to blit a texture to a canvas. The language side is covered in [Compute shaders](../language/compute.md); this page is the API reference.

Samples (user-facing code in each README / file):

| Sample | Role |
|--------|------|
| [`saxpy.dream`](https://github.com/sps014/dream/tree/main/sample/compute/saxpy.dream) | Beginner — one `@compute` + readback |
| [`gpu_ext.dream`](https://github.com/sps014/dream/tree/main/sample/compute/gpu_ext.dream) | Pass / indirect / `@readonly` |
| [`life/`](https://github.com/sps014/dream/tree/main/sample/compute/life) | Complex — Gray–Scott reaction–diffusion |
| [`fluid/`](https://github.com/sps014/dream/tree/main/sample/fluid) | Interactive stable fluids |

## Device

Probe and initialize the GPU once at startup. Without a successful `try_init`, dispatches and buffer uploads will fail or no-op depending on the host.

#### `Gpu.is_available(): bool`

Sync probe for a WebGPU adapter (`navigator.gpu` in the browser). Use it to decide whether to take a GPU path or fall back to CPU before spending work on init.

```dream
if (!Gpu.is_available()) {
    System.println("no GPU");
}
```

#### `await Gpu.try_init(): Result<bool, GpuError>`

Requests adapter and device (idempotent). Call once before the first dispatch or upload. Returns `Err` when the device is missing, timed out, or rejected validation.

```dream
switch (await Gpu.try_init()) {
    Ok(_) => {},
    Err(e) => System.println(e.message()),
}
```

#### `Gpu.ready(): bool`

True after a successful `try_init`. Prefer this over repeating `is_available` once you have started the GPU path.

```dream
System.println(Gpu.ready());
```

#### `GpuError`

Implements [`Error`](option-result.md) with `message()` / `code()`. Factories (`unavailable`, `timeout`, `validation`, `other`, `from_code`) are for constructing errors in host bridges and tests — application code usually reads them from `Result`.

```dream
let e = GpuError.unavailable("no adapter");
System.println(e.code());
```

## Timing

Frame pacing and clocks for interactive demos. Prefer `Gpu.frame` when drawing to a canvas so work lines up with the display; use `Time.delay` / `Time.sleep` for fixed-rate simulation steps that are not tied to vsync.

#### `await Time.delay(ms: int): void`

Wall-clock delay (browser `setTimeout`). Use for real-time pacing that should match wall time even if the tab is busy.

```dream
await Time.delay(16);
```

#### `await Time.sleep(ms: int): void`

Cooperative / virtual-clock sleep on Dream's scheduler. Use inside async host code when you want the event loop to keep running other tasks, or when the simulation clock is virtual rather than wall time.

```dream
await Time.sleep(100);
```

#### `await Gpu.frame(): void`

Waits for one display frame (`requestAnimationFrame`). Use at the end of a render loop so present + next tick stay in sync with the monitor.

```dream
await Gpu.frame();
```

#### `await Gpu.timestamp(): long`

Queue/host timestamp in nanoseconds. Use for GPU-side or host-adjacent profiling of a pass (not a substitute for `Stopwatch` on pure CPU work).

```dream
let ns = await Gpu.timestamp();
```

## Buffers

GPU storage for unmanaged element types (`float`, `int`, …). Kernels read and write `GpuBuffer<T>` by index; the CPU side uploads and readbacks through these methods. Prefer `.from` when you already have seed data; `.alloc` when the kernel fills the buffer.

`T` must be `unmanaged`. Staging uses `Bytes.of` / `Bytes.to`. `GpuBuffer` is a value `struct` handle (cheap to copy; the GPU resource is shared).

#### `GpuBuffer<T>.alloc(n: int): GpuBuffer<T>`

Allocates an uninitialized buffer of `n` elements on the GPU. Use for outputs or scratch that a kernel will fill.

```dream
let out = GpuBuffer<float>.alloc(1024);
```

#### `GpuBuffer<T>.from(data: T[]): GpuBuffer<T>`

Allocates and uploads `data` in one step. Use for constants and initial state.

```dream
let a = GpuBuffer<float>.from([1.0, 2.0, 3.0]);
```

#### `.length` / `.id` / `.stride`

Element count, host resource id, and byte stride. `length` is what kernels and dispatch extents usually need; `id` is for `run_resources` / low-level binding lists.

#### `write(data: T[]): void` / `write_at(offset: int, data: T[]): void`

CPU → GPU upload. `write` replaces the whole buffer (or as much as `data` covers from the start); `write_at` patches a slice. Use between frames when CPU simulation updates a region the next kernel reads.

```dream
a.write([4.0, 5.0, 6.0]);
a.write_at(1, [9.0]);
```

#### `await read(): T[]` / `await read_at(offset: int, count: int): T[]`

GPU → CPU readback. Expensive — use for debugging, final results, or infrequent sync points, not every frame in a hot loop.

```dream
let all = await a.read();
let slice = await a.read_at(0, 2);
```

#### `copy_to(dst, src_offset, dst_offset, count): void`

GPU-side element copy with no CPU round-trip. Use to duplicate or shift data between buffers before the next dispatch.

```dream
a.copy_to(out, 0, 0, 3);
```

#### `GpuSwap<T>.alloc(n)` / `front()` / `back()` / `swap()`

Ping-pong pair of buffers. Many simulations read from `front` and write to `back`, then `swap` so the next iteration flips roles without copying. Prefer this over manually juggling two `GpuBuffer`s.

```dream
let ping = GpuSwap<float>.alloc(1024);
let src = ping.front();
let dst = ping.back();
// ... dispatch reading src, writing dst ...
ping.swap();
```

In `@compute` kernels, storage params are **`GpuBuffer<T>`** (not bare `T[]`); index with `buf[i]` and use `buf.length`. Mark inputs `@readonly` so WGSL emits `var<storage, read>`:

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

Launch named `@compute` kernels. Pass `GpuBuffer`s in the same order as the kernel's buffer parameters. Use `run_1d` for linear data, `run_2d`/`run_3d` for grids, and `ComputePass` when several dispatches should share one queue submit.

#### `await Compute.run_1d(name, buffers, count)`

Dispatches enough workgroups to cover `count` 1D threads. Default choice for array-shaped kernels.

```dream
let r = await Compute.run_1d("scale", [a, out], 3);
```

#### `await Compute.run_2d(name, buffers, width, height)` / `run_3d(...)`

2D/3D grid extents. Use for image-like or volume domains where `global_id.xy` / `.xyz` matter.

```dream
let r2 = await Compute.run_2d("blur", [tex_in, tex_out], w, h);
```

#### `await Compute.run_2d_uniforms(name, buffers, width, height, uniforms)`

Same as `run_2d`, then binds an extra uniform blob (after the extent i32s). Use when the kernel needs small constants (timesteps, radii) without packing them into a storage buffer.

#### `await Compute.run_resources(...)`

Explicit buffer / texture / sampler id lists when binding order is not a plain `GpuBuffer[]` — e.g. kernels that mix storage buffers with sampled textures.

#### `await Compute.dispatch_indirect(name, buffers, indirect: GpuBuffer<int>)`

`dispatchWorkgroupsIndirect` — workgroup counts come from a GPU buffer (3× u32). Use when a prior kernel decides how much work the next pass needs.

#### `await Compute.run_shader(shader, ...)`

Runs a raw [`GpuShader`](#gpushaderfrom_wgsl) instead of a Dream `@compute` name. Escape hatch for hand-written WGSL.

#### `Uniforms.pack_i32(values)` / `pack_f32(values)`

Packs a small uniform payload as bytes for `run_*_uniforms` / `dispatch_uniforms`. Prefer these over hand-rolling endian layouts.

```dream
let u = Uniforms.pack_f32([1.0, 2.0]);
```

#### `ComputePass.begin()` / `dispatch(...)` / `dispatch_uniforms` / `dispatch_resources` / `dispatch_indirect` / `await submit()`

Batches several dispatches into one queue submit. Use when a frame has multiple kernels that should run back-to-back with less host overhead than separate `Compute.run_*` calls.

```dream
let pass = ComputePass.begin();
pass.dispatch("scale", [a, out], 3, 1, 1);
switch (await pass.submit()) {
    Ok(_) => {},
    Err(e) => System.println(e.message()),
}
```

#### `GpuDispatchIndirect(x, y, z)` / `to_buffer()` / `write_to(buf)`

Helper that packs workgroup counts into a `GpuBuffer<int>` for `dispatch_indirect`. Use when CPU code knows the counts; write them from a kernel when the GPU decides.

```dream
let counts = GpuDispatchIndirect(8, 1, 1);
let buf = counts.to_buffer();
counts.write_to(buf);
```

#### `GpuShader.from_wgsl(source, entry)`

Compiles raw WGSL for `Compute.run_shader`. Prefer Dream `@compute` kernels unless you need WGSL features the Dream frontend does not expose yet.

```dream
let shader = GpuShader.from_wgsl("@compute @workgroup_size(64) fn main() {}", "main");
```

## Textures / samplers / present

Image-shaped GPU resources for compute that samples or stores pixels, and for presenting to a canvas. Pair a sampled `@readonly GpuTexture` with a `GpuSampler` in the kernel signature.

#### `GpuTexture.rgba8(width, height)`

Creates an RGBA8 texture. Starting point for fluid/life-style demos that store color or packed state per pixel.

```dream
let tex = GpuTexture.rgba8(w, h);
```

#### `await write_rgba(pixels)` / `await write_rgba_at(...)` / `await read_rgba()`

CPU ↔ texture pixel transfers (`byte[]` RGBA). Use `write_*` to seed from CPU; `read_rgba` sparingly for screenshots or debugging.

```dream
await tex.write_rgba(rgba_bytes);
let pixels = await tex.read_rgba();
```

#### `copy_from_buffer` / `copy_to_buffer`

GPU-side copies between a `GpuBuffer<byte>` and a texture. Prefer these over readback+rewrite when moving packed pixel data entirely on the GPU.

#### `GpuSampler.linear()` / `nearest()`

Sampling state for compute. `linear` for smooth interpolation; `nearest` for exact texel fetches (cellular automata, integer grids).

```dream
let samp = GpuSampler.linear();
```

#### `GpuSurface.from_canvas(canvas_id)` / `configure(w, h)` / `await present()`

Swapchain for an HTML canvas. `configure` when the canvas size changes; `present` after blitting the frame you want shown.

#### `await GpuRenderPass.blit(surface, tex)`

Fullscreen blit of a texture onto the surface. Typical end of an interactive frame: compute → texture → blit → present → `Gpu.frame`.

```dream
switch (GpuSurface.from_canvas("fluid")) {
    Ok(surface) => {
        surface.configure(w, h);
        await GpuRenderPass.blit(surface, tex);
        await surface.present();
        await Gpu.frame();
    },
    Err(e) => System.println(e.message()),
}
```

#### Kernel texture builtins

- `Gpu.texture_load(tex, x, y)` — read a texel (storage / load path).
- `Gpu.texture_store(tex, x, y, r, g, b, a)` — write a texel.
- `Gpu.texture_sample_level(tex, samp, u, v, level)` — filtered sample with a sampler.

Use load/store for integer grid updates; sample when you need interpolation.

## `GpuMath` (kernel)

Float math that lowers to WGSL builtins inside `@compute` bodies. Prefer these over host `Math.*` — host math is not available in kernels, and `GpuMath` keeps types as `float`.

Available: `min`, `max`, `abs`, `clamp`, `floor`, `ceil`, `sqrt`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`.

```dream
@compute(64)
fun soften(out: GpuBuffer<float>, n: int): void {
    let i = global_id.x;
    out[i] = GpuMath.clamp(out[i], 0.0, 1.0);
}
```

#### `Gpu.workgroup_barrier()` / `Gpu.storage_barrier()`

Synchronize threads in a workgroup or memory visibility for storage buffers. Use when threads in the same dispatch share intermediate values and must wait for each other's writes before reading them.

## Atomics (kernel-only)

#### `Gpu.atomic_load` / `atomic_store` / `atomic_add` / `atomic_exchange`

Cross-thread atomic ops on `GpuBuffer<int>` (emitted as `array<atomic<i32>>`). Use for counters, flags, and reductions where many threads update the same slot.

```dream
@compute(64)
fun count(flags: GpuBuffer<int>, n: int): void {
    let _ = Gpu.atomic_add(flags, 0, 1);
}
```

## Native vs browser

| Host | Behavior |
|------|----------|
| Browser (`dream.js`) | Real WebGPU when available |
| Native (`dream run`) | CPU staging; dispatch no-ops WGSL; surface/blit unavailable |

See [Compute shaders](../language/compute.md).
