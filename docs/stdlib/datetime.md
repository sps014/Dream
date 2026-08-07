# DateTime

**Package:** `system` — `import system;`

`DateTime` is an instant, rendered in UTC or a fixed local offset. The same package also provides `Time` and `Stopwatch` (also summarized in [Built-ins](builtins.md)).

Only wall clock and local timezone offset need the host. Calendar math, arithmetic, comparison, and ISO-8601 are pure Dream.

```dream
import system;

fun main(): void {
    let now = DateTime.now();
    System.println(now.to_iso8601());

    let launch = DateTime.of(2026, 7, 2, 9, 30, 0, 0);
    System.println(launch.add_days(7).to_iso8601());
}
```

## Construction

#### `DateTime.utc_now(): DateTime`

Returns the current instant in UTC. Use when you need a timezone-neutral timestamp for storage or APIs that expect `Z`.

```dream
let u = DateTime.utc_now();
```

#### `DateTime.now(): DateTime`

Returns the current instant with the host's local timezone offset attached. Prefer for user-facing "what time is it here?" displays.

```dream
let n = DateTime.now();
```

#### `DateTime.from_epoch_millis(millis: long): DateTime`

Builds a `DateTime` from Unix epoch milliseconds (UTC). Use when ingesting timestamps from databases, JSON, or other systems.

```dream
let dt = DateTime.from_epoch_millis(0L);
```

#### `DateTime.of(year, month, day, hour, minute, second, millisecond): DateTime`

Constructs a UTC calendar instant from field components. Use for fixed deadlines, test fixtures, or parsing-free construction.

```dream
let launch = DateTime.of(2026, 7, 2, 9, 30, 0, 0);
```

#### `DateTime.of_local(...)`

Same field components interpreted as local wall-clock time on the host. Use when the user enters "9:30 AM" in their timezone rather than UTC.

```dream
let local = DateTime.of_local(2026, 7, 2, 9, 30, 0, 0);
```

#### `DateTime(epoch_millis, offset_minutes)`

Low-level constructor from raw epoch millis and a fixed offset in minutes. Rarely needed — prefer `of` / `of_local` unless reconstructing from stored parts.

```dream
let raw = DateTime(0L, 0);
```

## Fields and calendar accessors

#### `.epoch_millis` / `.offset_minutes`

Exposes the underlying instant and fixed offset. `epoch_millis` is the absolute time; `offset_minutes` is how local fields relate to UTC for this value.

```dream
let dt = DateTime.now();
System.println(dt.epoch_millis);
System.println(dt.offset_minutes);
```

#### `year()` / `month()` / `day()`

Returns calendar date parts in this value's offset (local fields when offset is local). Use for display and date-only logic without string parsing.

```dream
System.println(dt.year());
System.println(dt.month());
System.println(dt.day());
```

#### `hour()` / `minute()` / `second()` / `millisecond()`

Returns time-of-day parts in this value's offset. Sub-second precision comes from `millisecond()` (0–999).

```dream
System.println(dt.hour());
System.println(dt.minute());
System.println(dt.second());
System.println(dt.millisecond());
```

#### `day_of_week(): int`

Returns the weekday as `0` = Sunday … `6` = Saturday in the value's offset. Use for scheduling rules ("run on Mondays").

```dream
System.println(dt.day_of_week());
```

#### `day_of_year(): int`

Returns the 1-based day index within the year (Jan 1st is `1`). Handy for ordinal dates and year-fraction calculations.

```dream
System.println(dt.day_of_year());
```

#### `decompose(): DateTimeYmd`

Returns the internal year/month/day breakdown struct. Rarely needed in application code — prefer the individual accessors unless interfacing with low-level calendar code.

```dream
let ymd = dt.decompose();
```

## Conversion

#### `to_utc(): DateTime`

Re-expresses the same instant with UTC offset (`0`). Use before serializing to APIs that require `Z` suffixes.

```dream
System.println(dt.to_utc().to_iso8601());
```

#### `to_local(): DateTime`

Re-resolves the host's local offset for this instant (DST-correct). Call after arithmetic that may cross a DST boundary so displayed local time stays correct.

```dream
System.println(dt.to_local().to_iso8601());
```

## Arithmetic

Each takes a `long` and returns a new `DateTime` with the same `offset_minutes`. Call `.to_local()` afterwards if you need a DST-correct offset after crossing a boundary.

#### `add_millis` / `add_seconds` / `add_minutes` / `add_hours` / `add_days`

Shifts the instant forward or backward by the given unit (negative values go backward). All return a new value — `DateTime` is immutable.

```dream
let dt = DateTime.of(2026, 7, 2, 10, 0, 0, 0);
let tomorrow = dt.add_days(1);
let an_hour_ago = dt.add_hours(0L - 1L);
let later = dt.add_minutes(30L).add_seconds(15L).add_millis(250L);
```

## Comparison

Compares absolute instant (`epoch_millis`), ignoring `offset_minutes`.

#### `compare_to(other): int` / `is_before` / `is_after` / `equals`

Orders or tests two instants by absolute time. `compare_to` returns `< 0`, `0`, or `> 0` — the others are readable shortcuts.

```dream
let a = DateTime.of(2026, 1, 1, 0, 0, 0, 0);
let b = DateTime.of(2026, 6, 1, 0, 0, 0, 0);
System.println(a.is_before(b));   // true
System.println(b.is_after(a));    // true
System.println(a.compare_to(b));  // -1
System.println(a.equals(a));      // true
```

## Formatting and parsing

#### `to_iso8601(): string`

Formats as `"YYYY-MM-DDTHH:mm:ss.fffZ"` or with `±HH:MM` offset. Prefer for machine-readable interchange and log timestamps.

```dream
System.println(DateTime.utc_now().to_iso8601());
```

#### `to_string(): string`

Human-readable local display (space-separated, no fractional seconds). Use in UI where ISO punctuation is too noisy.

```dream
System.println(DateTime.now().to_string());
```

#### `DateTime.parse_iso8601(text): Result<DateTime, ParseError>`

Parses ISO-8601 text into a `DateTime`. Missing fraction defaults to `0`; missing offset defaults to UTC.

```dream
let parsed = DateTime.parse_iso8601("2026-07-02T10:35:00.250Z");
System.println(parsed.unwrap_or(DateTime.from_epoch_millis(0L)).to_iso8601());
```

## `Time`

#### `await Time.delay(ms: int): void`

Pauses for real wall-clock milliseconds (browser `setTimeout`). Use when pacing must match real time even if the tab is busy.

```dream
await Time.delay(16);
```

#### `await Time.sleep(ms: int): void`

Cooperative sleep on Dream's async scheduler. Prefer inside async code when other tasks should keep running, or when using a virtual simulation clock.

```dream
await Time.sleep(100);
```

#### `Time.nano_time(): long`

Returns monotonic nanoseconds since an unspecified origin. Use for high-resolution elapsed timing without calendar semantics.

```dream
let t0 = Time.nano_time();
```

## `Stopwatch`

#### `Stopwatch()` / `start()` / `stop()` / `reset()` / `restart()`

Measures elapsed time between `start` and `stop`. `restart` clears and starts in one step; `reset` clears without starting.

```dream
let sw = Stopwatch();
sw.start();
await Time.delay(5);
sw.stop();
System.println(sw.elapsed_ms());
System.println(sw.elapsed_nanos());
sw.restart();
System.println(sw.is_running);  // true
sw.reset();
```

## Platform notes

| Runtime | Wall clock | Local timezone |
| --- | --- | --- |
| Native (`dream run`) | OS system clock | OS timezone database (DST-aware) |
| Node.js / browser | `Date.now()` | `Date.getTimezoneOffset()` |
