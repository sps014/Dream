# Compute shaders (`@compute`)

Dream can compile ordinary-looking functions into **WebGPU compute shaders** (WGSL). Mark a
top-level function with `@compute` and dispatch it through `system.gpu` — no bind-group
boilerplate for the common case.

You can start with a one-kernel SAXPY, build a reaction–diffusion sim, or study the full
fluid demo. Native `dream run` stages GPU buffers but does **not** execute WGSL; use a
browser with WebGPU for real GPU results (see [stdlib GPU](../stdlib/gpu.md)).

## Quick start

User-facing code — a kernel plus a tiny host dispatch:

```dream
import system;
import system.gpu;

@compute(64)
fun add(a: GpuBuffer<float>, b: GpuBuffer<float>, out: GpuBuffer<float>, n: int): void {
    let i = global_id.x;
    if (i < a.length && i < n) {
        out[i] = a[i] + b[i];
    }
}

async fun main(): void {
    let init = await Gpu.try_init();
    if (init.is_err()) { return; }
    let a = GpuBuffer<float>.from([1.0, 2.0, 3.0]);
    let b = GpuBuffer<float>.from([10.0, 20.0, 30.0]);
    let out = GpuBuffer<float>.alloc(3);
    let r = await Compute.run_1d("add", [a, b, out], 3);
    if (r.is_err()) { return; }
    let vals = await out.read();
    System.println((int)vals[0]); // browser: 11
}
```

Compiling emits a sibling `.wgsl` file and a `"gpu"` section in `.abi.json`. The browser
runtime (`runtime/dream.js`) loads both and drives `navigator.gpu`.

## Simple sample: SAXPY

[`sample/compute/saxpy.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/saxpy.dream)
— one kernel, one dispatch, readback:

```dream
import system;
import system.gpu;

@compute(64)
fun saxpy(x: GpuBuffer<float>, y: GpuBuffer<float>, out: GpuBuffer<float>, n: int): void {
    let i = global_id.x;
    if (i < n) {
        out[i] = 2.0 * x[i] + y[i];
    }
}

async fun main(): void {
    let init = await Gpu.try_init();
    if (init.is_err()) { return; }
    let x = GpuBuffer<float>.from([1.0, 2.0, 3.0, 4.0]);
    let y = GpuBuffer<float>.from([10.0, 20.0, 30.0, 40.0]);
    let out = GpuBuffer<float>.alloc(4);
    let _ = await Compute.run_1d("saxpy", [x, y, out], 4);
    let vals = await out.read();
    System.println((int)vals[0]); // browser: 12
}
```

```bash
cargo run -- run sample/compute/saxpy.dream
```

## Complex sample: reaction–diffusion

[`sample/compute/life/`](https://github.com/sps014/Dream/tree/main/sample/compute/life) —
Gray–Scott chemical simulation (not cellular automata): dual fields, multi-step
`ComputePass`, texture paint.

```dream
@compute(8, 8)
fun rd_step(
    @readonly u_in: GpuBuffer<float>,
    @readonly v_in: GpuBuffer<float>,
    u_out: GpuBuffer<float>,
    v_out: GpuBuffer<float>,
    n: int, _ey: int, _ez: int,
    du: float, dv: float, feed: float, kill: float
): void {
    // … laplacian(u/v) + u·v² reaction …
}

@compute(8, 8)
fun rd_paint(
    @readonly u: GpuBuffer<float>,
    @readonly v: GpuBuffer<float>,
    tex: GpuTexture,
    n: int
): void {
    Gpu.texture_store(tex, global_id.x, global_id.y, /* palette from v */);
}
```

Host batch (browser path):

```dream
let pass = ComputePass.begin();
// several rd_step dispatches (ping-pong), then:
pass.dispatch_resources(
    "rd_paint",
    [u.id, v.id],
    [tex.id],
    Buffer.alloc<int>(0),
    n, n, 1,
    Buffer.alloc<byte>(0)
);
let _ = await pass.submit();
await GpuRenderPass.blit(surface, tex);
```

```bash
cargo run -- sample/compute/life/life.dream
# serve repo root → sample/compute/life/life.html
```

## Larger demo: fluid

[`sample/fluid/`](https://github.com/sps014/Dream/tree/main/sample/fluid) — Jos Stam–style
2D stable fluids on the GPU: `ComputePass` batches splat / advect / Jacobi project / decay /
paint; the CPU only tracks mouse + frame.

```dream
@compute(8, 8)
fun advect(
    @readonly src: GpuBuffer<float>,
    dst: GpuBuffer<float>,
    @readonly vx: GpuBuffer<float>,
    @readonly vy: GpuBuffer<float>,
    n: int
): void {
    let x = global_id.x;
    let y = global_id.y;
    if (x >= n || y >= n) { return; }
    // … bilinear sample of src at (x - dt*vx, y - dt*vy) …
}

// Host: several ComputePass submits per frame, then blit.
let pass = ComputePass.begin();
pass.dispatch("advect", [vx, vx_tmp, vx, vy], n, n, 1);
pass.dispatch("advect", [vy, vy_tmp, vx, vy], n, n, 1);
let _ = await pass.submit();
```

```bash
cargo run -- sample/fluid/fluid.dream
# serve repo root → sample/fluid/fluid.html
```

## Attribute

| Form | Meaning |
|------|---------|
| `@compute` | Workgroup size `(64, 1, 1)` |
| `@compute(x)` | `(x, 1, 1)` |
| `@compute(x, y)` | `(x, y, 1)` |
| `@compute(x, y, z)` | Full 3D workgroup |

Only **top-level** `fun`s may carry `@compute`. Kernels must return `void`, cannot be
`async`/`extern`/generic, and are **not** callable as CPU functions — use
`Compute.run_1d` / `Compute.run_2d` with the kernel **name**.

## Storage parameters

Kernel storage buffers are **`GpuBuffer<T>`** (not bare `T[]`). Inside a kernel you can
index them (`a[i]`) and read **`a.length`** (WGSL `arrayLength`). Scalars and unmanaged
value structs become uniforms. The host packs dispatch extents `ex, ey, ez` into the first
three `i32` slots of that uniform block (so a trailing `n: int` often matches the grid size).

Prefix a buffer (or texture) with **`@readonly`** for WGSL `var<storage, read>` / sampled
`texture_2d` instead of `read_write` / storage-texture write:

```dream
@compute(64)
fun scale(@readonly a: GpuBuffer<float>, out: GpuBuffer<float>, n: int): void {
    let i = global_id.x;
    if (i < n) { out[i] = a[i] * 2.0; }
}
```

Host dispatch still passes `GpuBuffer` instances to `Compute.run_*` in binding order.
Kernels may also take `GpuTexture` / `GpuSampler`; use `Compute.run_resources` or
`ComputePass.dispatch_resources` to supply their host ids.

## Builtins

Inside a kernel, these locals are in scope (typed as `GpuId3` with `.x`/`.y`/`.z`):

- `global_id` — global invocation id
- `local_id` — local invocation id
- `workgroup_id` — workgroup id
- `num_workgroups` — dispatch size in workgroups

## Language surface

Allowed: `if`/`else`, `while`/`do`/`for`, `break`/`continue` (including labels), early
`return`, ternary, integer `switch`, arithmetic/bitwise, `GpuBuffer` indexing / `.length`,
unmanaged value structs, calls to other `@compute` helpers, `Gpu.workgroup_barrier` /
`Gpu.storage_barrier`, `Gpu.atomic_*`, `Gpu.texture_*`, `GpuMath.*`.

Forbidden: bare `T[]` as a kernel param, `string`/`List`/`class`/`js`/`async`, `for..in`,
union pattern-match `switch`, `lock`, recursion, calling ordinary CPU functions.

### Workgroup memory

```dream
@compute(64)
fun reduce(data: GpuBuffer<float>, out: GpuBuffer<float>): void {
    @workgroup(64) let tile: float;
    let lid = local_id.x;
    tile[lid] = data[global_id.x];
    Gpu.workgroup_barrier();
    // …
}
```

`@workgroup(N) let name: T;` becomes WGSL `var<workgroup> name: array<T, N>`.

### `@shared` is not GPU shared memory

Dream's existing `@shared` attribute marks **CPU / WebWorker** heap classes (lock word +
atomic RC). It is illegal inside `@compute`. GPU scratch uses `@workgroup`, not `@shared`.

## Multi-pass sync

WebGPU has **no** global barrier across workgroups. Algorithms that need one (e.g. Jacobi
pressure solve) issue multiple dispatches; host queue order provides happens-before.

Prefer **`ComputePass`** to batch several dispatches into one `queue.submit`:

```dream
let pass = ComputePass.begin();
pass.dispatch("advect", [src, dst, vx, vy], n, n, 1);
pass.dispatch("divergence", [vx, vy, div], n, n, 1);
let _ = await pass.submit();
```

For GPU-written workgroup counts, pack three i32s with `GpuDispatchIndirect` and call
`Compute.dispatch_indirect` (or `pass.dispatch_indirect`).

## Escape hatch

```dream
let shader = GpuShader.from_wgsl(WGSL_SOURCE, "main");
let r = await Compute.run_shader(shader, [buf], 64, 1, 1);
```

## Samples

| Sample | Role |
|--------|------|
| [`sample/compute/saxpy.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/saxpy.dream) | Beginner — one kernel + readback |
| [`sample/compute/gpu_ext.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/gpu_ext.dream) | API surface — `@readonly`, `ComputePass`, indirect |
| [`sample/compute/life/`](https://github.com/sps014/Dream/tree/main/sample/compute/life) | Complex — Gray–Scott reaction–diffusion |
| [`sample/fluid/`](https://github.com/sps014/Dream/tree/main/sample/fluid) | Larger demo — interactive stable fluids |

## See also

- [stdlib GPU](../stdlib/gpu.md)
- Beginner: [`saxpy.dream`](https://github.com/sps014/Dream/tree/main/sample/compute/saxpy.dream)
- Complex: [`sample/compute/life/`](https://github.com/sps014/Dream/tree/main/sample/compute/life)
- Fluid: [`sample/fluid/`](https://github.com/sps014/Dream/tree/main/sample/fluid)
