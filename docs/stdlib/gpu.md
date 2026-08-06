# `system.gpu`

WebGPU compute and present from Dream. Auto-imported when any `@compute` kernel is present
(same pattern as `@json` → `system.json`). You can also `import system.gpu;`.

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
| `write` / `write_at` | Full / partial CPU→GPU upload |
| `read` / `read_at` | Full / partial readback |
| `GpuSwap<T>` | Ping-pong `front` / `back` / `swap` |

`T` must be `unmanaged`. Staging uses `Bytes.of` / `Bytes.to`.

```dream
let a = GpuBuffer<float>.from([1.0, 2.0, 3.0]);
let out = GpuBuffer<float>.alloc(3);
let _ = await Gpu.try_init();
let r = await Compute.run_1d("add", [a, /*…*/, out], 3);
```

## Dispatch

| API | Role |
|-----|------|
| `Compute.run_1d/2d/3d` | Named `@compute` kernels; returns `Result` |
| `Compute.run_2d_uniforms` | Extra uniform blob after extent i32s |
| `Uniforms.pack_i32` / `pack_f32` | Build uniform bytes |
| `GpuShader` + `Compute.run_shader` | Raw WGSL escape hatch |

Pass `GpuBuffer<float>[]` in kernel binding order (no raw buffer ids).

## Textures / present

| API | Role |
|-----|------|
| `GpuTexture.rgba8` | RGBA8 texture |
| `write_rgba` / `write_rgba_at` / `read_rgba` | Upload / readback |
| `GpuSurface.from_canvas` | Canvas swapchain |
| `configure` / `present` | Resize / present |
| `GpuRenderPass.blit` | Fullscreen blit texture → surface |

```dream
let surface = GpuSurface.from_canvas("fluid").unwrap_or(/*…*/);
let tex = GpuTexture.rgba8(w, h);
await tex.write_rgba(rgba_bytes);
await GpuRenderPass.blit(surface, tex);
await surface.present();
await Gpu.frame();
```

## Native vs browser

| Host | Behavior |
|------|----------|
| Browser (`dream.js`) | Real WebGPU when available |
| Native (`dream run`) | CPU staging; dispatch no-ops WGSL; surface/blit unavailable |

See [Compute shaders](../language/compute.md).
