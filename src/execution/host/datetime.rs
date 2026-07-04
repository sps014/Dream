//! Wall-clock host functions (the `Dream` module behind `src/stdlib/system/datetime.dream`).
//! Calendar math itself is implemented in pure Dream; this only bridges the two things that
//! genuinely require the host: the current time and the OS timezone database. Browser/Node hosts
//! implement the same names in `runtime/dream.js`.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use wasmtime::*;

static START_INSTANT: OnceLock<Instant> = OnceLock::new();

/// Registers the `DateTime` host functions on `linker`. Shared by the CLI runner and the E2E test
/// harness so the native behavior can never drift.
pub fn link_datetime_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap("Dream", "dateNowMillis", || -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    })?;

    linker.func_wrap("Dream", "timeNowNanos", || -> i64 {
        let start = START_INSTANT.get_or_init(Instant::now);
        start.elapsed().as_nanos() as i64
    })?;

    // Minutes *east* of UTC for the local system timezone at the given UTC epoch millisecond
    // instant (e.g. IST is +330, PST is -480), accounting for DST. `runtime/dream.js` mirrors this
    // with the opposite-signed `Date.getTimezoneOffset()`, negated to match this convention.
    linker.func_wrap("Dream", "dateLocalOffsetMinutes", |millis: i64| -> i32 {
        use chrono::{Local, TimeZone};
        match Local.timestamp_millis_opt(millis) {
            chrono::LocalResult::Single(dt) => dt.offset().local_minus_utc() / 60,
            chrono::LocalResult::Ambiguous(dt, _) => dt.offset().local_minus_utc() / 60,
            chrono::LocalResult::None => 0,
        }
    })?;

    Ok(())
}
