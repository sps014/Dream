# Dream fluid (WebGPU)

Jos Stam–style 2D stable fluids written as Dream `@compute` kernels and dispatched through
`system.gpu`. For a smaller multi-kernel GPU demo, see
[`sample/compute/life/`](../compute/life/) (Game of Life). The browser host in
[`runtime/dream.js`](../../runtime/dream.js) loads the sibling `.wgsl` + `.abi.json` and runs
them on WebGPU.

## User-facing kernels

```dream
@compute(8, 8)
fun advect(
    src: GpuBuffer<float>,
    dst: GpuBuffer<float>,
    vx: GpuBuffer<float>,
    vy: GpuBuffer<float>,
    n: int
): void {
    let x = global_id.x;
    let y = global_id.y;
    if (x >= n || y >= n) { return; }
    // … bilinear sample …
}
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

Requires a modern browser with WebGPU. The live loop is a CPU Stam-style sim (auto orbit +
drag splat) presented via `GpuTexture` + `GpuRenderPass.blit` + `GpuSurface`. `@compute`
kernels are still compiled to WGSL in the sibling `.wgsl` / `.abi.json`. Native `dream run`
stages GPU host stubs without drawing (see `docs/stdlib/gpu.md`).
