# Compute samples

Progressive `@compute` / `system.gpu` examples. Language guide:
[Compute shaders](../../docs/language/compute.md).

| Sample | Role |
|--------|------|
| [`saxpy.dream`](saxpy.dream) | Beginner — SAXPY kernel + readback |
| [`gpu_ext.dream`](gpu_ext.dream) | `@readonly`, `ComputePass`, indirect dispatch |
| [`life/`](life/) | Complex — Conway's Game of Life (GPU + canvas) |
| [`../fluid/`](../fluid/) | Larger demo — interactive stable fluids |

```bash
cargo run -- run sample/compute/saxpy.dream
cargo run -- run sample/compute/life/life.dream
```
