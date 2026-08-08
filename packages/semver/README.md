# semver

Parse, compare, and match [Semantic Versioning](https://semver.org/) requirements in Dream.
Useful for feature gates, plugin loaders, and any code that speaks the same version language as
`dream.toml` / `dreamer`.

## Install

```bash
dreamer add semver --version "^0.1"
```

```toml
[dependencies]
semver = "0.1"
```

## Usage

```dream
import system;
import system.text;
import semver;

fun main(): void {
    let v = SemVer.parse("1.2.3-beta.1+build").unwrap_or(SemVer(0, 0, 0, "", ""));
    System.println(v.to_string());
    System.println(v.major.to_string() + "." + v.minor.to_string() + "." + v.patch.to_string());

    let a = SemVer.parse("1.2.0").unwrap_or(SemVer(0, 0, 0, "", ""));
    let b = SemVer.parse("1.10.0").unwrap_or(SemVer(0, 0, 0, "", ""));
    System.println(SemVer.compare(a, b).to_string()); // -1
    System.println(a.lt(b));                           // true

    System.println(SemVer.satisfies(v, "^1.2"));              // true
    System.println(SemVer.satisfies(v, "~1.2.0"));             // true (same major.minor)
    System.println(SemVer.satisfies(v, ">=1.0.0, <2.0.0"));    // true
    System.println(SemVer.satisfies(v, "1.2.3"));              // false (exact; prerelease ≠ release)
}
```

## API

```dream
public class SemVer {
    public major: int;
    public minor: int;
    public patch: int;
    public prerelease: string;   // "" if none
    public build: string;        // "" if none

    public static fun parse(s: string): Result<SemVer, string>;
    public fun to_string(): string;

    public static fun compare(a: SemVer, b: SemVer): int;
    public fun eq(other: SemVer): bool;
    public fun lt(other: SemVer): bool;
    public fun lte(other: SemVer): bool;
    public fun gt(other: SemVer): bool;
    public fun gte(other: SemVer): bool;

    /// Supports exact (`1.2.3`), `^`, `~`, `>=` / `>` / `<=` / `<`,
    /// and comma-separated AND lists (`>=1.0.0, <2.0.0`).
    public static fun satisfies(version: SemVer, req: string): bool;
}
```

Build metadata is ignored for precedence (SemVer §10). A release version is greater than any
prerelease with the same numeric triple.
