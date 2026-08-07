# Dream fluid (WebGPU)

Jos Stam–style 2D stable fluids. The sim runs on the GPU: splat → advect → project
(Jacobi) → decay → paint, batched with `ComputePass` (a few submits per frame, no CPU
field loops or RGBA upload). For a smaller multi-kernel demo, see
[`sample/compute/life/`](../compute/life/) (Gray–Scott reaction–diffusion).

## User-facing kernels

```dream
@compute(8, 8)
fun advect(
    @readonly src: GpuBuffer<float>,
    dst: GpuBuffer<float>,
    @readonly vx: GpuBuffer<float>,
    @readonly vy: GpuBuffer<float>,
    n: int
): void { /* bilinear back-trace */ }

@compute(8, 8)
fun paint(
    @readonly dens: GpuBuffer<float>,
    @readonly vx: GpuBuffer<float>,
    @readonly vy: GpuBuffer<float>,
    tex: GpuTexture,
    n: int
): void { /* Gpu.texture_store palette */ }
```

Host batch (sketch):

```dream
let pass = ComputePass.begin();
pass.dispatch_uniforms("splat", [dens, vx, vy], n, n, 1, splat_u);
pass.dispatch("advect", [vx, vx_tmp, vx, vy], n, n, 1);
pass.dispatch("advect", [vy, vy_tmp, vx, vy], n, n, 1);
let _ = await pass.submit();
// … project / jacobi / dye / decay / paint in further passes …
await GpuRenderPass.blit(surface, tex);
```

## Build

```sh
cargo run -- sample/fluid/fluid.dream
```

## Run

Serve the **repository root** (so `../../runtime/dream.js` resolves):

```sh
npx serve .
# open http://localhost:3000/sample/fluid/fluid.html
```

Requires a modern browser with WebGPU. Drag to paint; an auto orbit also injects force.
Native `dream run` cannot present a canvas (see `docs/stdlib/gpu.md`).
