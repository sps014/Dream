# Game of Life (WebGPU)

Conway's Game of Life as Dream `@compute` kernels: a 2D `life_step` ping-pong plus
`life_paint` into a `GpuTexture`, batched with `ComputePass`. Distinct from the
[fluid](../../fluid/) sample (stable fluids / mouse paint).

## User-facing kernels

```dream
@compute(8, 8)
fun life_step(@readonly cur: GpuBuffer<float>, next: GpuBuffer<float>, n: int): void {
    // … neighbor count, B3/S23 rules …
}

@compute(8, 8)
fun life_paint(@readonly cells: GpuBuffer<float>, tex: GpuTexture, n: int): void {
    // … Gpu.texture_store …
}
```

## Build

```sh
cargo run -- sample/compute/life/life.dream
```

## Run

Native / headless (`dream run`) prints an ASCII CPU demo (WebGPU dispatch is a no-op on
native; kernels still emit WGSL):

```sh
cargo run -- run sample/compute/life/life.dream
```

Browser — serve the **repository root**:

```sh
npx serve .
# open http://localhost:3000/sample/compute/life/life.html
```

Requires WebGPU. See [Compute shaders](../../../docs/language/compute.md).
