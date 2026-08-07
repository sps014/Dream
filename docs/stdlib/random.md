# Random

**Package:** `system` — `import system;`

Seedable PRNG for non-cryptographic use (games, tests, shuffling). For cryptographic randomness, use [`SecureRandom`](crypto.md).

```dream
import system;

fun main(): void {
    let rng = Random(42u);
    System.println(rng.next_int(10));
    System.println(rng.next_bool());
}
```

#### `Random(seed: uint)`

Creates a deterministic PRNG from a seed (`0` is remapped to `1`). Use fixed seeds in tests for reproducible sequences.

```dream
let rng = Random(42u);
```

#### `next_u32(): uint`

Returns the next raw 32-bit value from the generator. Building block for custom distributions — most code uses `next_int` or `next_double` instead.

```dream
let u = rng.next_u32();
```

#### `next_int(bound: int): int`

Returns a uniform integer in `[0, bound)` when `bound > 0`. Standard choice for dice rolls, array indices, and bounded game logic.

```dream
System.println(rng.next_int(10));  // 0..9
```

#### `next_double(): double`

Returns a uniform double in `[0.0, 1.0)`. Use for probabilities and floating-point jitter.

```dream
System.println(rng.next_double());
```

#### `next_bool(): bool`

Returns `true` or `false` with equal probability. Shortcut for coin flips and random branching in simulations.

```dream
System.println(rng.next_bool());
```
