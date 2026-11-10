//! Stage-attributed benchmark for the metadata → information schema conversion.
//!
//! `--generate-info-schema` is opt-in because its cost is unknown. Two decisions
//! need numbers: whether the conversion can be on by default, and whether a
//! DuckDB-side materializer should replace the Arrow one. [`run_with`] times
//! each path on the same corpus; the report attributes wall time to stages and
//! as a fraction of the invocation that produced the metadata. The default-flag
//! decision turns on *cold* conversion as a share of compile, not the steady
//! delta path.
//!
//! This is not shipped library code: the module is gated behind `cfg(test)` and
//! the `bench` feature (see `info_schema/mod.rs`). A plain `#[cfg(test)]` would
//! not do — the e2e harness in `dbt-cli` generates the metadata corpus and then
//! calls [`run`] across the crate boundary, where the `test` cfg does not apply,
//! so it enables the `bench` feature on its dev-dependency instead.
//!
//! Iterations are not equivalent, and the report keeps them apart. The ingest
//! persists its state into the staging directory, so only the first conversion
//! walks every epoch; later ones find no new epochs and take the delta path,
//! while the projection re-runs in full every time. Both are production
//! behaviour — a clean checkout, CI and `dbt clean` all make a run the cold one —
//! so collapsing them into one mean would have hidden the entire ingest cost.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::IndexError;
use crate::ingest::metadata_to_parquet::{f64_col, read_parquet_batches};
use crate::ingest::timings::{self, Stage};

use super::{Materializer, STAGING_DIR_NAME, versioned_dir, write_info_schema_with};

/// One conversion of a fixed metadata directory.
pub struct Iteration {
    /// The output directory did not exist beforehand, so this is the conversion a
    /// project pays the first time — the only one that walks every epoch.
    pub cold: bool,
    /// Wall time of the whole conversion, ingest and projection together.
    pub wall: Duration,
    /// Tables written.
    pub tables: usize,
    /// Per-stage attribution, in [`Stage::ALL`] order.
    pub stages: Vec<(Stage, Duration, u64)>,
}

impl Iteration {
    /// Time and call count attributed to one stage.
    pub fn stage(&self, stage: Stage) -> (Duration, u64) {
        self.stages
            .iter()
            .find(|(s, _, _)| *s == stage)
            .map(|(_, d, c)| (*d, *c))
            .unwrap_or((Duration::ZERO, 0))
    }

    /// Sum of the attributed stages. Stages are disjoint (see
    /// [`crate::ingest::timings`]), so the remainder against [`Iteration::wall`]
    /// is genuinely uninstrumented work rather than double counting.
    pub fn attributed(&self) -> Duration {
        self.stages.iter().map(|(_, d, _)| *d).sum()
    }

    /// Wall time not accounted for by any stage.
    pub fn unattributed(&self) -> Duration {
        self.wall.saturating_sub(self.attributed())
    }
}

/// The result of a benchmark run against one metadata corpus.
pub struct Report {
    /// Project or corpus name, for the report header.
    pub label: String,
    /// Which materializer produced these numbers.
    pub materializer: Materializer,
    pub metadata_dir: PathBuf,
    pub iterations: Vec<Iteration>,
    /// `elapsed_time` of the longest invocation recorded in the metadata, in
    /// seconds — the denominator for "what fraction of a command is this".
    /// `None` when the metadata carries no invocation row, as after a bare
    /// `parse`.
    pub invocation_secs: Option<f64>,
}

/// Convert `metadata_dir` into an information schema under `workdir`
/// `iterations` times, timing each one. Uses [`Materializer::Arrow`], the
/// historical path the recorded numbers were taken on.
///
/// `workdir` stands in for a target directory: the output lands in
/// `workdir/info_schema/v<n>/` and the staging tables in
/// `workdir/<STAGING_DIR_NAME>/`. It is neither created empty nor cleaned
/// between iterations, so iteration 1 measures a first run and the rest measure
/// the steady state.
pub fn run(
    label: impl Into<String>,
    metadata_dir: &Path,
    workdir: &Path,
    iterations: usize,
) -> Result<Report, IndexError> {
    run_with(
        Materializer::Arrow,
        label,
        metadata_dir,
        workdir,
        iterations,
    )
}

/// [`run`] with an explicit materializer, so Arrow and COPY can be timed on
/// the same corpus without one path's leftover staging affecting the other.
pub fn run_with(
    how: Materializer,
    label: impl Into<String>,
    metadata_dir: &Path,
    workdir: &Path,
    iterations: usize,
) -> Result<Report, IndexError> {
    let info_schema_dir = workdir.join(super::INFO_SCHEMA_DIR_NAME);
    let staging_dir = workdir.join(STAGING_DIR_NAME);
    let out_dir = versioned_dir(&info_schema_dir);

    let mut runs = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let cold = !out_dir.exists();
        timings::reset();
        let start = Instant::now();
        let tables = write_info_schema_with(how, metadata_dir, &info_schema_dir, &staging_dir)?;
        let wall = start.elapsed();
        runs.push(Iteration {
            cold,
            wall,
            tables,
            stages: timings::snapshot(),
        });
    }

    Ok(Report {
        label: label.into(),
        materializer: how,
        metadata_dir: metadata_dir.to_path_buf(),
        invocation_secs: longest_invocation_secs(&out_dir),
        iterations: runs,
    })
}

/// The longest `elapsed_time` in `dbt_rt.invocations`, which the conversion has
/// just written. Read from the output rather than the epoch files so it needs no
/// knowledge of the epoch layout; a missing or empty table is not an error.
fn longest_invocation_secs(out_dir: &Path) -> Option<f64> {
    let batches = read_parquet_batches(&out_dir.join("dbt_rt.invocations.parquet")).ok()?;
    let mut best: Option<f64> = None;
    for batch in &batches {
        let Some(col) = f64_col(batch, "elapsed_time") else {
            continue;
        };
        for row in 0..batch.num_rows() {
            if arrow_array::Array::is_null(col, row) {
                continue;
            }
            let v = col.value(row);
            if best.is_none_or(|b| v > b) {
                best = Some(v);
            }
        }
    }
    best
}

impl Report {
    /// The first conversion, which found no output directory: a project's
    /// first-ever `--generate-info-schema`. It is the only iteration that pays a
    /// full epoch ingest, because the ingest persists its state into the staging
    /// directory and every later run takes the delta path from it.
    pub fn cold(&self) -> Option<&Iteration> {
        self.iterations.first().filter(|i| i.cold)
    }

    /// The steady-state iterations — every one but the first. Falls back to the
    /// single cold iteration when that is all there is.
    pub fn steady(&self) -> &[Iteration] {
        if self.iterations.len() > 1 {
            &self.iterations[1..]
        } else {
            &self.iterations
        }
    }

    /// Mean steady-state wall time.
    pub fn mean_wall(&self) -> Duration {
        mean(self.steady().iter().map(|i| i.wall))
    }

    /// Mean steady-state time for one stage.
    pub fn mean_stage(&self, stage: Stage) -> Duration {
        mean(self.steady().iter().map(|i| i.stage(stage).0))
    }

    /// Calls to one stage across every iteration. A zero for
    /// [`Stage::EpochRead`] means the conversion never opened an epoch file, so
    /// the corpus was empty and every number in the report is measuring nothing.
    pub fn calls(&self, stage: Stage) -> u64 {
        self.iterations.iter().map(|i| i.stage(stage).1).sum()
    }

    /// Conversion cost as a fraction of the command that produced the metadata.
    /// Steady-state wall over invocation elapsed time.
    pub fn fraction_of_invocation(&self) -> Option<f64> {
        let secs = self.invocation_secs.filter(|s| *s > 0.0)?;
        Some(self.mean_wall().as_secs_f64() / secs)
    }

    /// Cold conversion as a fraction of the command that produced the metadata.
    /// This is the number the opt-in-default decision turns on.
    pub fn cold_fraction_of_invocation(&self) -> Option<f64> {
        let secs = self.invocation_secs.filter(|s| *s > 0.0)?;
        Some(self.cold()?.wall.as_secs_f64() / secs)
    }

    /// A pasteable report: one row per stage with the cold run beside the
    /// steady-state mean.
    ///
    /// Both columns are needed because they are different code paths, not the
    /// same path warming up. The cold run does the full epoch ingest; the steady
    /// runs take the delta path and re-project, so a stage that dominates one can
    /// be absent from the other.
    pub fn table(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let steady_wall = self.mean_wall();
        let cold_wall = self.cold().map(|i| i.wall).unwrap_or_default();
        let tables = self.iterations.first().map(|i| i.tables).unwrap_or(0);

        let _ = writeln!(
            out,
            "info schema conversion ({}) — {}",
            self.materializer.label(),
            self.label
        );
        let _ = writeln!(out, "  metadata:   {}", self.metadata_dir.display());
        let _ = writeln!(
            out,
            "  iterations: {} ({} tables per conversion)",
            self.iterations.len(),
            tables
        );
        match (
            self.invocation_secs,
            self.cold_fraction_of_invocation(),
            self.fraction_of_invocation(),
        ) {
            (Some(secs), Some(cold_frac), Some(steady_frac)) => {
                let _ = writeln!(
                    out,
                    "  invocation: {:.1} ms → cold is {:.1}% of it, steady is {:.1}%",
                    secs * 1000.0,
                    cold_frac * 100.0,
                    steady_frac * 100.0
                );
            }
            _ => {
                let _ = writeln!(out, "  invocation: not recorded in this metadata");
            }
        }

        let _ = writeln!(
            out,
            "  {:<14} {:>9} {:>7} {:>9} {:>7} {:>6}",
            "stage", "cold ms", "share", "steady ms", "share", "calls"
        );
        let row = |out: &mut String, label: &str, cold: Duration, steady: Duration, calls: &str| {
            let _ = writeln!(
                out,
                "  {:<14} {:>9.1} {:>6.1}% {:>9.1} {:>6.1}% {:>6}",
                label,
                ms(cold),
                share(cold, cold_wall),
                ms(steady),
                share(steady, steady_wall),
                calls
            );
        };
        for stage in Stage::ALL {
            let cold = self.cold().map(|i| i.stage(*stage).0).unwrap_or_default();
            let calls = self
                .cold()
                .map(|i| i.stage(*stage).1)
                .unwrap_or_else(|| self.calls(*stage));
            row(
                &mut out,
                stage.label(),
                cold,
                self.mean_stage(*stage),
                &calls.to_string(),
            );
        }
        row(
            &mut out,
            "(other)",
            self.cold().map(|i| i.unattributed()).unwrap_or_default(),
            mean(self.steady().iter().map(|i| i.unattributed())),
            "",
        );
        row(&mut out, "= wall", cold_wall, steady_wall, "");
        out
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn share(part: Duration, whole: Duration) -> f64 {
    if whole.is_zero() {
        0.0
    } else {
        part.as_secs_f64() / whole.as_secs_f64() * 100.0
    }
}

fn mean(values: impl Iterator<Item = Duration>) -> Duration {
    let mut total = Duration::ZERO;
    let mut n = 0u32;
    for v in values {
        total += v;
        n += 1;
    }
    if n == 0 { Duration::ZERO } else { total / n }
}
