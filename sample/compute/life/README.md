# Reaction–diffusion (WebGPU)

Gray–Scott continuous chemical simulation: fields `u` / `v` diffuse and react on the GPU
(`rd_step`), then `rd_paint` writes a storage texture. Eight steps are batched per frame in
one `ComputePass`. This replaces the old Game of Life demo with a richer evolving system.

## User-facing kernels

```dream
@compute(8, 8)
fun rd_step(
    @readonly u_in: GpuBuffer<float>,
    @readonly v_in: GpuBuffer<float>,
    u_out: GpuBuffer<float>,
    v_out: GpuBuffer<float>,
    n: int, _ey: int, _ez: int,
    du: float, dv: float, feed: float, kill: float
): void { /* laplacian + u·v² reaction */ }

@compute(8, 8)
fun rd_paint(
    @readonly u: GpuBuffer<float>,
    @readonly v: GpuBuffer<float>,
    tex: GpuTexture,
    n: int
): void { /* Gpu.texture_store */ }
```

## Build / run

```sh
cargo run -- sample/compute/life/life.dream
npx serve .
# open http://localhost:3000/sample/compute/life/life.html
```

Requires WebGPU. See [Compute shaders](../../../docs/language/compute.md).
