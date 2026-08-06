# Random

**Package:** `system` — `import system;`

`Random` is a seedable PRNG for non-cryptographic use (games, tests, shuffling). For cryptographic randomness, use [`system.crypto`](crypto.md) `SecureRandom`.

| Member | Description |
| --- | --- |
| `Random(seed)` | construct; seed `0` is remapped to `1` |
| `next_u32()` | next 32-bit value |
| `next_int(bound)` | uniform in `[0, bound)` when `bound > 0` |
| `next_double()` | uniform in `[0.0, 1.0)` |
| `next_bool()` | true/false with equal probability |

```dream
import system;

fun main(): void {
    let rng = Random(42u);
    System.println(rng.next_int(10));
    System.println(rng.next_bool());
}
```
