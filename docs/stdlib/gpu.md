# `system.gpu`

WebGPU compute from Dream. Auto-imported when any `@compute` kernel is present (same pattern as `@json` → `system.json`). You can also `import system.gpu;`.

## Types

| Type | Role |
|------|------|
| `GpuId3` | `{ x, y, z: int }` — kernel builtins |
| `GpuBuffer<T : unmanaged>` | Storage buffer of blittable `T` |
| `GpuShader` | Raw WGSL module (`from_wgsl`) |
| `Compute` | `run_1d` / `run_2d` / `run_shader` |
| `Gpu` | Host bridges + barriers |
| `GpuMath` | `min`/`max`/`abs`/`clamp`/`floor`/`ceil`/`sqrt`/`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`atan2` |

## Buffers

```dream
let a = GpuBuffer<float>.from([1.0, 2.0, 3.0]);
let out = GpuBuffer<float>.alloc(3);
await Compute.run_1d("add", [a.buffer_id(), /*…*/], 3);
let result = await out.read();
```

`T` must be `unmanaged` (primitives or blittable value structs). Staging uses `Bytes.of` / `Bytes.to` under the hood.

## Present to canvas

```dream
await Gpu.present_rgba("fluid", rgba_bytes, width, height);
```

Implemented in `runtime/dream.js` via Canvas2D `ImageData` (looks up the canvas by id). Native `dream run` resolves the call without drawing.

## Native vs browser

| Host | Behavior |
|------|----------|
| Browser (`dream.js`) | Real WebGPU when `navigator.gpu` exists |
| Native (`dream run`) | CPU staging buffers; `gpuDispatch` resolves without executing WGSL (use the browser for real shaders) |

See [Compute shaders](../language/compute.md).
