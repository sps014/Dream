# Compute shaders (`@compute`)

Dream can compile ordinary-looking functions into **WebGPU compute shaders** (WGSL). Mark a top-level function with `@compute` and dispatch it through `system.gpu` — no bind-group boilerplate required for the common case.

## Quick start

```dream
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
    print(await out.read());
}
```

Compiling emits a sibling `.wgsl` file and a `"gpu"` section in `.abi.json`. The browser runtime (`runtime/dream.js`) loads both and drives `navigator.gpu`.

## Attribute

| Form | Meaning |
|------|---------|
| `@compute` | Workgroup size `(64, 1, 1)` |
| `@compute(x)` | `(x, 1, 1)` |
| `@compute(x, y)` | `(x, y, 1)` |
| `@compute(x, y, z)` | Full 3D workgroup |

Only **top-level** `fun`s may carry `@compute`. Kernels must return `void`, cannot be `async`/`extern`/generic, and are **not** callable as CPU functions — use `Compute.run_1d` / `Compute.run_2d` with the kernel **name**.

## Storage parameters

Kernel storage buffers are **`GpuBuffer<T>`** (not bare `T[]`). Inside a kernel you can index them (`a[i]`) and read **`a.length`** (WGSL `arrayLength`). Scalars and unmanaged value structs become uniforms.

Host dispatch still passes `GpuBuffer` instances to `Compute.run_*` in binding order.

## Builtins

Inside a kernel, these locals are in scope (typed as `GpuId3` with `.x`/`.y`/`.z`):

- `global_id` — global invocation id  
- `local_id` — local invocation id  
- `workgroup_id` — workgroup id  
- `num_workgroups` — dispatch size in workgroups  

## Language surface

Allowed: `if`/`else`, `while`/`do`/`for`, `break`/`continue` (including labels), early `return`, ternary, integer `switch`, arithmetic/bitwise, `GpuBuffer` indexing / `.length`, unmanaged value structs, calls to other `@compute` helpers, `Gpu.workgroup_barrier` / `Gpu.storage_barrier`, `GpuMath.*`.

Forbidden: bare `T[]` as a kernel param, `string`/`List`/`class`/`js`/`async`, `for..in`, union pattern-match `switch`, `lock`, recursion, calling ordinary CPU functions.

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

Dream's existing `@shared` attribute marks **CPU / WebWorker** heap classes (lock word + atomic RC). It is illegal inside `@compute`. GPU scratch uses `@workgroup`, not `@shared`.

## Multi-pass sync

WebGPU has **no** global barrier across workgroups. Algorithms that need one (e.g. Jacobi pressure solve) issue multiple `await Compute.run_…` calls; host queue order provides happens-before.

## Escape hatch

```dream
let shader = GpuShader.from_wgsl(WGSL_SOURCE, "main");
let r = await Compute.run_shader(shader, [buf], 64, 1, 1);
```

See also [stdlib GPU](../stdlib/gpu.md).
