//! Stage attribution for the metadata → information schema conversion.
//!
//! The conversion is opt-in behind `--generate-info-schema` because its cost is
//! unknown. Deciding whether it can be on by default needs cost attributed to
//! stages, not just a total: the read, the `payload` JSON parse, the row build
//! and the write have very different fixes, and which of them dominates decides
//! whether a DuckDB-side materializer would beat the Arrow path. (Measured, it is
//! the row build and the parquet write, not the JSON parse — see
//! `info_schema::bench`.)
//!
//! Accumulators are process-global atomics rather than a value threaded through
//! the ingest, because the ingest is ~3k lines across a dozen writers and
//! threading a parameter through all of them to serve a benchmark would be a
//! worse trade. Recording happens per *batch*, never per row, so the cost is
//! invisible next to the work being measured.
//!
//! # Stages are disjoint
//!
//! [`time`] attributes to a stage the time spent in a region *minus* whatever
//! other stages recorded inside it, and a region nested inside its own stage
//! records nothing. So stages never overlap: their sum is a lower bound on the
//! conversion's wall time and the remainder is genuinely uninstrumented work.
//! That is what makes the "(other)" line in a report meaningful rather than an
//! artifact of double counting.
//!
//! The subtraction reads the same global counters the nested regions write, so it
//! is only exact while the region under measurement is single-threaded — which
//! the conversion is. Concurrent work elsewhere in the process would be charged
//! against whatever region is open here.
//!
//! Only the benchmark harness reads these. A normal invocation writes to them
//! and never looks.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One attributable stage of the conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Globbing and reading epoch parquet into Arrow batches.
    EpochRead,
    /// Parsing `parse/nodes.payload` JSON.
    PayloadParse,
    /// The ingest itself: building source-shaped rows from Arrow batches and
    /// parsed payloads, merging them, pruning by liveness. Measured as the whole
    /// ingest minus the read, parse and write stages inside it, so it covers
    /// every table rather than only the instrumented ones.
    RowBuild,
    /// Serialising the staging (source-shaped) parquet.
    StagingWrite,
    /// Projecting staging tables into the information schema shape.
    Projection,
    /// DuckDB `COPY` of each public table from the epoch-view layer. Mutually
    /// exclusive with the ingest and projection stages: a conversion uses one
    /// path or the other.
    Copy,
    /// Generating `views.sql`.
    ViewsSql,
}

impl Stage {
    pub const ALL: &'static [Stage] = &[
        Stage::EpochRead,
        Stage::PayloadParse,
        Stage::RowBuild,
        Stage::StagingWrite,
        Stage::Projection,
        Stage::Copy,
        Stage::ViewsSql,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Stage::EpochRead => "epoch_read",
            Stage::PayloadParse => "payload_parse",
            Stage::RowBuild => "row_build",
            Stage::StagingWrite => "staging_write",
            Stage::Projection => "projection",
            Stage::Copy => "copy",
            Stage::ViewsSql => "views_sql",
        }
    }

    const fn index(self) -> usize {
        match self {
            Stage::EpochRead => 0,
            Stage::PayloadParse => 1,
            Stage::RowBuild => 2,
            Stage::StagingWrite => 3,
            Stage::Projection => 4,
            Stage::Copy => 5,
            Stage::ViewsSql => 6,
        }
    }
}

const N: usize = 7;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);

static NANOS: [AtomicU64; N] = [ZERO; N];
static CALLS: [AtomicU64; N] = [ZERO; N];

thread_local! {
    /// Which stages have a region open on this thread, so a stage nested inside
    /// itself is charged once — by its outermost region.
    static OPEN: [Cell<bool>; N] = const { [const { Cell::new(false) }; N] };
}

/// Add an elapsed duration to a stage.
pub fn record(stage: Stage, elapsed: Duration) {
    let i = stage.index();
    NANOS[i].fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
    CALLS[i].fetch_add(1, Ordering::Relaxed);
}

/// Time `f` and attribute it to `stage`, exclusive of every other stage recorded
/// inside it. A call nested inside an open region of the same stage is timed by
/// that outer region and adds nothing here.
pub fn time<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    if OPEN.with(|open| open[stage.index()].replace(true)) {
        return f();
    }
    // Closed even if `f` panics, so one failure does not silently mis-attribute
    // the rest of the run.
    struct Close(usize);
    impl Drop for Close {
        fn drop(&mut self) {
            OPEN.with(|open| open[self.0].set(false));
        }
    }
    let _close = Close(stage.index());

    let before = attributed_except(stage);
    let start = Instant::now();
    let out = f();
    let elapsed = start.elapsed();
    let nested = attributed_except(stage).saturating_sub(before);
    record(stage, elapsed.saturating_sub(nested));
    out
}

/// Time recorded against every stage but `stage`.
fn attributed_except(stage: Stage) -> Duration {
    let skip = stage.index();
    let nanos: u64 = (0..N)
        .filter(|i| *i != skip)
        .map(|i| NANOS[i].load(Ordering::Relaxed))
        .sum();
    Duration::from_nanos(nanos)
}

/// Total time and call count attributed to a stage so far.
pub fn read(stage: Stage) -> (Duration, u64) {
    let i = stage.index();
    (
        Duration::from_nanos(NANOS[i].load(Ordering::Relaxed)),
        CALLS[i].load(Ordering::Relaxed),
    )
}

/// Zero every accumulator. Call between benchmark iterations.
pub fn reset() {
    for i in 0..N {
        NANOS[i].store(0, Ordering::Relaxed);
        CALLS[i].store(0, Ordering::Relaxed);
    }
}

/// A snapshot of every stage, for reporting.
pub fn snapshot() -> Vec<(Stage, Duration, u64)> {
    Stage::ALL
        .iter()
        .map(|s| {
            let (d, c) = read(*s);
            (*s, d, c)
        })
        .collect()
}

/// Sum of every stage. Stages are disjoint, so this is a lower bound on the
/// conversion's wall time and the difference is uninstrumented work.
pub fn total() -> Duration {
    Stage::ALL.iter().map(|s| read(*s).0).sum()
}

/// One line per stage: `stage<TAB>millis<TAB>calls`. Stable enough to diff
/// between runs and paste into a PR.
pub fn report() -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (stage, dur, calls) in snapshot() {
        writeln!(
            out,
            "{}\t{:.3}\t{}",
            stage.label(),
            dur.as_secs_f64() * 1000.0,
            calls
        )
        .unwrap();
    }
    out
}

/// The accumulators are process-global, so these tests would interfere with any
/// other test that ingests in the same process: the crate's ingest tests call
/// [`time`] concurrently and corrupt the counters. cargo-nextest gives every
/// test its own process (the proprietary CI path), so they are exact there;
/// plain `cargo test` (the copybara OSS check) shares one process, so they
/// self-skip rather than fail spuriously.
#[cfg(test)]
mod tests {
    use super::*;

    /// True when each test runs in its own process. cargo-nextest sets `NEXTEST`
    /// in every test process; plain `cargo test` does not.
    fn process_isolated() -> bool {
        std::env::var_os("NEXTEST").is_some()
    }

    /// `reset` clears, `time` attributes to the stage it is given, and the
    /// report covers every stage even at zero.
    #[test]
    fn records_resets_and_reports() {
        if !process_isolated() {
            eprintln!(
                "skipping records_resets_and_reports: needs per-test process isolation (cargo-nextest)"
            );
            return;
        }
        reset();
        assert_eq!(read(Stage::PayloadParse), (Duration::ZERO, 0));

        time(Stage::PayloadParse, || {
            std::thread::sleep(Duration::from_millis(2))
        });
        let (dur, calls) = read(Stage::PayloadParse);
        assert_eq!(calls, 1);
        assert!(dur >= Duration::from_millis(1), "recorded {dur:?}");
        assert_eq!(read(Stage::EpochRead), (Duration::ZERO, 0));

        assert_eq!(report().lines().count(), Stage::ALL.len());

        reset();
        assert_eq!(total(), Duration::ZERO);
    }

    /// A nested stage is charged to itself and subtracted from its parent, and a
    /// stage nested inside itself is charged once. Without both, the "(other)"
    /// remainder in a report is meaningless.
    #[test]
    fn stages_are_disjoint() {
        if !process_isolated() {
            eprintln!(
                "skipping stages_are_disjoint: needs per-test process isolation (cargo-nextest)"
            );
            return;
        }
        reset();
        time(Stage::RowBuild, || {
            // Same stage re-entered: the inner region must add nothing.
            time(Stage::RowBuild, || {
                std::thread::sleep(Duration::from_millis(5));
            });
            time(Stage::StagingWrite, || {
                std::thread::sleep(Duration::from_millis(20));
            });
        });

        let (outer, outer_calls) = read(Stage::RowBuild);
        let (inner, inner_calls) = read(Stage::StagingWrite);
        assert_eq!(
            outer_calls, 1,
            "the re-entered region recorded a second call"
        );
        assert_eq!(inner_calls, 1);
        assert!(inner >= Duration::from_millis(15), "inner was {inner:?}");
        assert!(
            outer < inner,
            "the nested stage was not subtracted: outer {outer:?}, inner {inner:?}"
        );
        reset();
    }
}
