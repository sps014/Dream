# Dream fluid (WebGPU)

Jos Stam–style 2D stable fluids written as Dream `@compute` kernels and dispatched through
`system.gpu`. The browser host in [`runtime/dream.js`](../../runtime/dream.js) loads the sibling
`.wgsl` + `.abi.json` and runs them on WebGPU.

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
