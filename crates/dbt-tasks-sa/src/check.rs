//! Project quality checks — runs user-authored SQL checks against the dbt index.
//!
//! Checks are `*.sql` files under `<project_root>/checks/`. Each file is a
//! query against parse-safe `dbt.*` views (via `info_schema()`). A query
//! returning zero rows passes; any returned rows are violations.
//!
//! Discovery and evaluation live here; `check_adapter` executes the SQL
//! against a published parse index.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Severity of a check. `Error` fails the run (non-zero exit); `Warn` reports
/// the violation but does not fail the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Error,
    Warn,
}

/// Which output column(s) a node `--select` scopes a check by.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SelectionFilter {
    /// Default: filter on `unique_id` if the check outputs that column,
    /// otherwise do not scope (whole project).
    #[default]
    Auto,
    /// Never scope — always the whole project (e.g. aggregate checks).
    None,
    /// Scope on these output columns: keep a row if the node id in ANY of them
    /// is in the selection. Each named column must exist in the check's output
    /// (else the check errors).
    Columns(Vec<String>),
}

/// The node set a check's violation rows should be scoped to, or `None` for no scoping.
///
/// One definition, used by the single parse-time runner. Earlier designs ran checks from two
/// places and diverged over scoping once, and the failure mode is a silent vacuous pass rather
/// than an error, so the rule lives in one place deliberately.
///
/// Returns `None` unless a selector was actually given. Scoping against the *default* selection is
/// wrong on both counts: it isn't a statement of intent, and it doesn't cover every node kind a check
/// may legitimately report on — `groups` and `macros` are absent from `Nodes::materializable_keys`,
/// so a "every group has an owner" check would have every violation filtered away.
///
/// Check ids are kept in the set: a check is a node in `dbt.nodes`/`dbt.checks` like any other, so
/// `--select some_check_name` must narrow a check-about-checks (reading `dbt.checks`) to that
/// check's own row, exactly like selecting a model narrows a check reading `dbt.models`. This
/// governs scoping only — which checks *run* is decided separately by `check_names`/`named` at the
/// call site.
pub fn scope_for_selection<'a>(
    selection_active: bool,
    selected_nodes: impl IntoIterator<Item = &'a String>,
) -> Option<BTreeSet<String>> {
    if !selection_active {
        return None;
    }
    Some(selected_nodes.into_iter().cloned().collect())
}

/// Whether a zero-row result means "nothing was examined" rather than "nothing is wrong".
///
/// Reporting the first as a pass claims a validation that never ran, which is the one outcome a
/// check must never produce. There are two ways to get there, and an empty scope is only the
/// obvious one:
///
/// - **Nothing selected at all.** Every row is filtered away because the scope is empty.
/// - **Nothing of the right *kind* selected.** `--select tag:quality` on a project that tags its
///   checks yields a scope of `check.*` ids. A check reporting `model.*` ids matches none of them,
///   so every row goes — but the scope is not empty, so the empty-scope test says "genuine pass".
///   Both are equally vacuous, and the second is reachable now that a named check can be scoped.
///
/// `reported_kinds` is the resource-type prefixes of the ids the check actually emitted in its
/// scoping columns, before filtering (see [`Evaluation::reported_kinds`]). Empty means the check
/// reported no rows anywhere, which is a real pass: the rule looked and found nothing.
pub fn zero_rows_is_vacuous(
    scope: Option<&BTreeSet<String>>,
    filter: &SelectionFilter,
    reported_kinds: &BTreeSet<String>,
) -> bool {
    // No selector: the whole project was examined.
    let Some(scope) = scope else { return false };
    // The check opted out of scoping, so the selection never touched its rows.
    if matches!(filter, SelectionFilter::None) {
        return false;
    }
    if scope.is_empty() {
        return true;
    }
    // The rule found nothing to report project-wide, so there was nothing for the scope to drop.
    if reported_kinds.is_empty() {
        return false;
    }
    !reported_kinds
        .iter()
        .any(|kind| scope.iter().any(|id| id.starts_with(kind)))
}

/// The resource-type prefix of a unique id: `model.p.a` -> `model.`.
///
/// Ids that carry no prefix are their own kind, so an exact match still works.
fn kind_of(id: &str) -> String {
    match id.find('.') {
        Some(i) => id[..=i].to_string(),
        None => id.to_string(),
    }
}

/// Map a check node's resolved `selection_filter_on` config onto a [`SelectionFilter`].
///
/// One definition for the single execution path; two paths previously disagreed
/// about what a check's rows are scoped by.
pub fn selection_filter_for(
    selection_filter_on: Option<&dbt_schemas::schemas::project::SelectionFilterOn>,
) -> SelectionFilter {
    use dbt_schemas::schemas::project::SelectionFilterOn;
    match selection_filter_on {
        None => SelectionFilter::Auto,
        Some(SelectionFilterOn::One(s)) if s.eq_ignore_ascii_case("none") => SelectionFilter::None,
        Some(SelectionFilterOn::One(s)) => SelectionFilter::Columns(vec![s.clone()]),
        Some(SelectionFilterOn::Many(v)) => SelectionFilter::Columns(v.clone()),
    }
}

// ---------------------------------------------------------------------------
// Result-batch evaluation
// ---------------------------------------------------------------------------

/// Why a check could not read the project metadata, in user-facing terms. `None` means it can.
///
/// Checks are **pure readers**: the views they query are declared over the epoch files this
/// invocation's parse already wrote, and nothing is converted or published on their behalf. An
/// earlier version read `target/private/index` instead and had each check catch that index up
/// itself, which went wrong three ways: concurrent checks rewrote the same parquet files while
/// siblings had them open (a non-deterministic `IO error: No such file or directory`),
/// `target/private/index/` appeared even when the invocation was never asked to write one, and
/// what it left there lacked the `.fusion_state.json` / `.artifact_meta.json` bookkeeping of a
/// published index — so anything reading `target/private/index` afterwards would treat a
/// half-built directory as real.
///
/// Asked *before* the views are built, not left to the first query, because a metadata directory
/// with no parse epochs publishes no views at all: every check would then fail to bind and report
/// its own binder error, twenty of which say less than one line naming the cause.
pub fn metadata_unavailable_reason(metadata_dir: &Path) -> Option<String> {
    if dbt_index_core::info_schema::epoch_views::parse_epochs_present(metadata_dir) {
        return None;
    }
    Some(format!(
        "no project metadata at {} — checks read the metadata this run parsed, and none was \
         written",
        metadata_dir.display()
    ))
}

/// Applied by the parse-time runner before any check executes. Guarding only one of two paths was
/// how a single invocation ended up skipping a parse-tier check for "no current metadata index"
/// while reporting a confident verdict from a compile-tier check against that same index.
/// Count violation rows in a check's result batches and build a `col=value, …`
/// preview (up to `max_rows`).
///
/// When `selected` is `Some`, rows are scoped to that node set according to `filter`, so a
/// project-wide check evaluates only the selected nodes. Shared by the standalone `dbt check`
/// parse-time runner; kept in one place so "what counts as a violation" has a single answer.
/// A check's rows, evaluated against the selection.
pub struct Evaluation {
    /// Rows that survived scoping. Zero is a pass -- unless it is vacuous; see
    /// [`zero_rows_is_vacuous`].
    pub violations: u64,
    /// First few rows as `col=value` text, for `run_results.json`.
    pub preview: Vec<String>,
    /// The same rows, unmodified, for display as a table. Every column the check
    /// selected, in the order it selected them -- `dbt show` shows a query's
    /// output, so a check's rows are shown the same way.
    pub preview_rows: Option<arrow::record_batch::RecordBatch>,
    /// Resource-type prefixes of the ids found in the scoping columns **before** filtering, so a
    /// zero-row result can be told apart from one whose scope held nothing of the right kind.
    pub reported_kinds: BTreeSet<String>,
    /// Whether the selection filtered these rows. False with no selection, and also when the check
    /// has nothing to scope by (`selection_filter_on: none`, or no `unique_id` under the default).
    pub scoped: bool,
}

/// Why a check could not be evaluated: the message shown to the user, and a fixed `reason` that
/// telemetry reports in its place.
#[derive(Debug)]
pub struct CheckError {
    pub reason: &'static str,
    pub message: String,
}

pub fn evaluate_batches(
    batches: &[arrow::record_batch::RecordBatch],
    filter: &SelectionFilter,
    selected: Option<&BTreeSet<String>>,
    max_rows: usize,
) -> Result<Evaluation, CheckError> {
    use arrow::util::display::ArrayFormatter;
    use dbt_index_core::format::FMT_OPTS;
    let arrow_error = |e: arrow::error::ArrowError| CheckError {
        reason: "evaluation_failed",
        message: e.to_string(),
    };
    let mut count: u64 = 0;
    let mut preview: Vec<String> = Vec::new();
    let mut reported_kinds: BTreeSet<String> = BTreeSet::new();
    let mut preview_slices: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut previewed = 0usize;
    let mut scoped = false;
    for batch in batches {
        let Ok(formatters) = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(c.as_ref(), &FMT_OPTS))
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        let schema = batch.schema();
        // Drop the redundant `name` column from previews when `unique_id` is
        // present — the id already identifies the node. Drop `message` always:
        // check SQL often aliases a prose column there, but the log preview is
        // the row's identity, not a second copy of that comment.
        let drop_name = schema.index_of("unique_id").is_ok();
        // Output column indices to match against the selection. Empty => no
        // filtering for this batch (count every row).
        let filter_idxs: Vec<usize> = match (selected, filter) {
            (None, _) | (Some(_), SelectionFilter::None) => Vec::new(),
            (Some(_), SelectionFilter::Auto) => {
                schema.index_of("unique_id").ok().into_iter().collect()
            }
            (Some(_), SelectionFilter::Columns(cols)) => {
                let mut idxs = Vec::with_capacity(cols.len());
                for c in cols {
                    match schema.index_of(c) {
                        Ok(i) => idxs.push(i),
                        Err(_) => {
                            return Err(CheckError {
                                reason: "selection_filter_invalid",
                                message: format!(
                                    "selection_filter_on: column '{c}' is not in the check's output columns"
                                ),
                            });
                        }
                    }
                }
                idxs
            }
        };
        scoped |= !filter_idxs.is_empty();
        let mut keep: Vec<u32> = Vec::new();
        for row in 0..batch.num_rows() {
            // Recorded before the row is filtered away: what the check reports on is a property of
            // the check, not of the selection.
            for &i in &filter_idxs {
                reported_kinds.insert(kind_of(&formatters[i].value(row).to_string()));
            }
            if let Some(sel) = selected
                && !filter_idxs.is_empty()
                && !filter_idxs
                    .iter()
                    .any(|&i| sel.contains(&formatters[i].value(row).to_string()))
            {
                continue;
            }
            count += 1;
            if previewed < max_rows {
                previewed += 1;
                keep.push(row as u32);
                let cells: Vec<String> = formatters
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        let n = schema.field(*i).name();
                        n != "message" && !(drop_name && n == "name")
                    })
                    .map(|(i, f)| format!("{}={}", schema.field(i).name(), f.value(row)))
                    .collect();
                if !cells.is_empty() {
                    preview.push(cells.join(", "));
                }
            }
        }
        // `take` rather than a slice: the surviving rows are not contiguous once a
        // selection has filtered some away.
        if !keep.is_empty() {
            let idx = arrow::array::UInt32Array::from(keep);
            let cols = batch
                .columns()
                .iter()
                .map(|c| arrow::compute::take(c.as_ref(), &idx, None))
                .collect::<Result<Vec<_>, _>>()
                .map_err(arrow_error)?;
            preview_slices.push(
                arrow::record_batch::RecordBatch::try_new(Arc::clone(&schema), cols)
                    .map_err(arrow_error)?,
            );
        }
    }
    let preview_rows = match preview_slices.split_first() {
        None => None,
        Some((first, _)) => arrow::compute::concat_batches(&first.schema(), &preview_slices).ok(),
    };
    Ok(Evaluation {
        violations: count,
        preview,
        preview_rows,
        reported_kinds,
        scoped,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── result severity ──────────────────────────────────────────────────────────

    fn check_with_severity(
        name: &str,
        severity: Option<dbt_schemas::schemas::common::Severity>,
    ) -> Arc<dbt_schemas::schemas::DbtCheck> {
        let mut c = dbt_schemas::schemas::DbtCheck::default();
        c.__common_attr__.unique_id = format!("check.p.{name}");
        c.__common_attr__.name = name.to_string();
        c.deprecated_config.severity = severity;
        Arc::new(c)
    }

    /// Every result carries the check's configured severity (defaulting to `error`), even one
    /// that could not be evaluated, so telemetry can report it alongside the status.
    #[test]
    fn results_carry_configured_severity() {
        use dbt_schemas::schemas::common::Severity;
        let checks = vec![
            check_with_severity("unset", None),
            check_with_severity("err", Some(Severity::Error)),
            check_with_severity("warn", Some(Severity::Warn)),
        ];
        let missing = std::env::temp_dir().join("dbt-check-severity-test-no-such-dir");
        let outcome = run_parse_time_checks(&checks, &missing, None, None, false);
        let got: Vec<_> = outcome
            .results
            .iter()
            .map(|r| (r.name.as_str(), r.status, r.severity))
            .collect();
        assert_eq!(
            got,
            vec![
                ("unset", "error", "error"),
                ("err", "error", "error"),
                ("warn", "error", "warn"),
            ]
        );
    }

    /// A check that never ran says why, and reports the selection as `unknown` rather than
    /// guessing whether it would have scoped the check's rows.
    #[test]
    fn unevaluated_results_carry_a_reason() {
        let checks = vec![check_with_severity("c", None)];
        let missing = std::env::temp_dir().join("dbt-check-reason-test-no-such-dir");
        let scope = BTreeSet::from(["model.p.a".to_string()]);

        let r = &run_parse_time_checks(&checks, &missing, Some(&scope), None, false).results[0];
        assert_eq!(
            (r.status, r.reason, r.selection),
            ("error", Some("metadata_unavailable"), "unknown")
        );
        let r = &run_parse_time_checks(&checks, &missing, None, None, false).results[0];
        assert_eq!(r.selection, "none");

        let r = &not_run_results(&checks, "skip_checks_flag", false)[0];
        assert_eq!(
            (r.status, r.severity, r.reason, r.selection),
            ("skipped", "error", Some("skip_checks_flag"), "none")
        );
    }

    // ── scoping ──────────────────────────────────────────────────────────────────

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// No selector means no scoping at all — *not* scoping against the default selection.
    ///
    /// The default selection is not a statement of intent, and `Nodes::materializable_keys` omits
    /// `groups` and `macros`, so scoping against it would filter away every violation from a check
    /// keyed on those (e.g. "every group has an owner") and report a vacuous pass.
    #[test]
    fn scope_is_none_without_a_selector() {
        let selected = ids(&["model.p.a", "group.p.analytics"]);
        assert_eq!(scope_for_selection(false, selected.iter()), None);
    }

    /// With a selector, scope to the selected nodes, checks included — a check is a node in
    /// `dbt.nodes`/`dbt.checks` like any other, so `--select some_check_name` must narrow a
    /// check-about-checks (reading `dbt.checks`) to that check's own row, the same way selecting a
    /// model narrows a check reading `dbt.models`. Which checks *run* is governed separately by
    /// `check_names`/`named` at the call site — this only ever scopes rows.
    #[test]
    fn scope_with_a_selector_includes_check_ids() {
        let selected = ids(&["model.p.a", "check.p.some_check", "seed.p.s"]);
        let scope = scope_for_selection(true, selected.iter()).expect("selector active");
        assert_eq!(
            scope,
            BTreeSet::from([
                "model.p.a".to_string(),
                "check.p.some_check".to_string(),
                "seed.p.s".to_string()
            ])
        );
    }

    /// A selector matching only checks scopes to exactly those checks — useful on its own now that
    /// a check can report on other checks, and still not a pass if nothing else matches: a per-node
    /// check reading `dbt.models` finds nothing in an all-checks scope and reports skipped, not pass.
    #[test]
    fn scope_can_contain_only_check_ids() {
        let selected = ids(&["check.p.one", "check.p.two"]);
        let scope = scope_for_selection(true, selected.iter()).expect("selector active");
        assert_eq!(
            scope,
            BTreeSet::from(["check.p.one".to_string(), "check.p.two".to_string()])
        );
    }

    /// A single-batch, all-Utf8 record batch from `(column, values)` pairs.
    fn preview_batch(cols: &[(&str, &[&str])]) -> arrow::record_batch::RecordBatch {
        use arrow::array::StringArray;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;
        let fields: Vec<Field> = cols
            .iter()
            .map(|(n, _)| Field::new(*n, DataType::Utf8, true))
            .collect();
        let arrays: Vec<Arc<dyn arrow::array::Array>> = cols
            .iter()
            .map(|(_, vals)| {
                Arc::new(StringArray::from(
                    vals.iter().map(|s| Some(*s)).collect::<Vec<_>>(),
                )) as _
            })
            .collect();
        arrow::record_batch::RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    /// Zero rows can mean "nothing was examined" two ways: nothing selected, or nothing of the
    /// kind this check reports on.
    #[test]
    fn zero_rows_is_vacuous_when_the_scope_holds_nothing_the_check_reports_on() {
        let empty = BTreeSet::new();
        let models = BTreeSet::from(["model.p.a".to_string()]);
        let checks_only = BTreeSet::from(["check.p.one".to_string(), "check.p.two".to_string()]);
        let reports_models = BTreeSet::from(["model.".to_string()]);
        let reports_checks = BTreeSet::from(["check.".to_string()]);
        let reports_nothing = BTreeSet::new();

        // Nothing selected -> vacuous, whatever the check reports on.
        assert!(zero_rows_is_vacuous(
            Some(&empty),
            &SelectionFilter::Auto,
            &reports_models
        ));
        assert!(zero_rows_is_vacuous(
            Some(&empty),
            &SelectionFilter::Columns(vec!["unique_id".to_string()]),
            &reports_models
        ));

        // The scope holds only checks and the check reports on models: every row was filtered
        // away, so nothing was examined. `--select tag:quality` on a project that tags its checks.
        assert!(zero_rows_is_vacuous(
            Some(&checks_only),
            &SelectionFilter::Auto,
            &reports_models
        ));

        // Same scope, but a check *about checks* — those ids are exactly what it reports on, so
        // zero rows means they are all fine.
        assert!(!zero_rows_is_vacuous(
            Some(&checks_only),
            &SelectionFilter::Auto,
            &reports_checks
        ));

        // A model was in scope and the check reports on models -> a genuine pass.
        assert!(!zero_rows_is_vacuous(
            Some(&models),
            &SelectionFilter::Auto,
            &reports_models
        ));

        // The check reported no rows at all: the rule looked project-wide and found nothing.
        assert!(!zero_rows_is_vacuous(
            Some(&models),
            &SelectionFilter::Auto,
            &reports_nothing
        ));

        // No selector at all -> whole project was examined.
        assert!(!zero_rows_is_vacuous(
            None,
            &SelectionFilter::Auto,
            &reports_models
        ));

        // `None` opts out of scoping, so the scope is irrelevant to it.
        assert!(!zero_rows_is_vacuous(
            Some(&empty),
            &SelectionFilter::None,
            &reports_models
        ));
    }

    /// The kinds are collected before filtering: what a check reports on is a property of the
    /// check, not of the selection that just discarded its rows.
    #[test]
    fn reported_kinds_survive_being_filtered_out() {
        let batch = preview_batch(&[("unique_id", &["model.p.a"]), ("name", &["a"])]);
        let scope = BTreeSet::from(["check.p.one".to_string()]);
        let eval = evaluate_batches(&[batch], &SelectionFilter::Auto, Some(&scope), 5).unwrap();

        assert_eq!(eval.violations, 0, "the model row is out of scope");
        assert_eq!(eval.reported_kinds, BTreeSet::from(["model.".to_string()]));
        assert!(zero_rows_is_vacuous(
            Some(&scope),
            &SelectionFilter::Auto,
            &eval.reported_kinds
        ));
    }

    /// `scoped` is set only when the selection actually filtered the rows. A check left at the
    /// default that outputs no `unique_id` has nothing to scope by, so a selector leaves it
    /// reporting on the whole project, which telemetry reports as `ignored`.
    #[test]
    fn scoped_only_when_the_selection_filters_rows() {
        let scope = BTreeSet::from(["model.p.a".to_string()]);
        let with_id = preview_batch(&[("unique_id", &["model.p.a"])]);
        let without_id = preview_batch(&[("name", &["a"])]);
        let scoped = |batch: &arrow::record_batch::RecordBatch,
                      filter: &SelectionFilter,
                      scope: Option<&BTreeSet<String>>| {
            evaluate_batches(std::slice::from_ref(batch), filter, scope, 5)
                .unwrap()
                .scoped
        };
        assert!(scoped(&with_id, &SelectionFilter::Auto, Some(&scope)));
        assert!(!scoped(&without_id, &SelectionFilter::Auto, Some(&scope)));
        assert!(!scoped(&with_id, &SelectionFilter::None, Some(&scope)));
        assert!(!scoped(&with_id, &SelectionFilter::Auto, None));
    }

    /// A `selection_filter_on` column the check does not output is the check's own config error,
    /// reported apart from a query that failed.
    #[test]
    fn a_missing_selection_filter_column_has_its_own_reason() {
        let batch = preview_batch(&[("unique_id", &["model.p.a"])]);
        let scope = BTreeSet::from(["model.p.a".to_string()]);
        let filter = SelectionFilter::Columns(vec!["parent_id".to_string()]);
        let error = evaluate_batches(&[batch], &filter, Some(&scope), 5)
            .err()
            .expect("parent_id is not in the check's output");
        assert_eq!(error.reason, "selection_filter_invalid");
    }

    #[test]
    fn preview_omits_name_and_message_when_unique_id_is_present() {
        let batch = preview_batch(&[
            ("unique_id", &["model.p.a"]),
            ("name", &["a"]),
            ("column_name", &["id"]),
            ("message", &["column has no description"]),
        ]);
        let eval = evaluate_batches(&[batch], &SelectionFilter::None, None, 5).unwrap();
        assert_eq!(eval.violations, 1);
        assert_eq!(
            eval.preview,
            vec!["unique_id=model.p.a, column_name=id".to_string()]
        );
    }

    /// The table shows the query's own output, so it keeps the columns the compact
    /// preview drops -- `message` in particular is usually the point of the rule,
    /// and it is the one column the `col=value` form always throws away.
    #[test]
    fn the_rows_table_keeps_every_column_the_check_selected() {
        let batch = preview_batch(&[
            ("unique_id", &["model.p.a"]),
            ("name", &["a"]),
            ("message", &["no description"]),
        ]);
        let eval = evaluate_batches(&[batch], &SelectionFilter::None, None, 5).unwrap();

        let rows = eval.preview_rows.expect("a violation row");
        assert_eq!(
            rows.schema()
                .fields()
                .iter()
                .map(|f| f.name().as_str())
                .collect::<Vec<_>>(),
            vec!["unique_id", "name", "message"],
        );
        let table = render_rows(&rows, eval.violations, Some(5), true).expect("a table");
        assert!(table.contains("no description"), "{table}");
        assert!(!table.contains("--limit"), "nothing was cut: {table}");
        assert!(table.contains("unique_id"), "header row missing: {table}");
    }

    /// Capped at the preview limit however many rows violate, and the rows kept are
    /// the ones that survived scoping -- which are not contiguous, so this cannot be
    /// a slice of the head of the batch.
    #[test]
    fn the_rows_table_is_capped_and_holds_only_selected_rows() {
        let batch = preview_batch(&[(
            "unique_id",
            &["model.p.a", "model.p.b", "model.p.c", "model.p.d"],
        )]);
        let scope = BTreeSet::from(["model.p.b".to_string(), "model.p.d".to_string()]);
        let eval = evaluate_batches(&[batch], &SelectionFilter::Auto, Some(&scope), 1).unwrap();

        assert_eq!(eval.violations, 2, "b and d are in scope");
        let rows = eval.preview_rows.expect("a violation row");
        assert_eq!(rows.num_rows(), 1, "capped at max_rows, not at violations");
        let table = render_rows(&rows, eval.violations, Some(1), true).expect("a table");
        assert!(table.contains("model.p.b"), "{table}");
        assert!(
            table.contains("2 rows, showing only 1 rows") && table.contains("--limit -1"),
            "a cut table must say how to see the rest: {table}"
        );
        let quiet = render_rows(&rows, eval.violations, Some(1), false).expect("a table");
        let exact = render_rows(&rows, 1, Some(1), true).expect("a table");
        assert!(
            !exact.contains("--limit"),
            "nothing cut when rows == limit: {exact}"
        );
        assert!(
            !quiet.contains("--limit"),
            "no hint for commands without the flag: {quiet}"
        );
        assert!(
            !table.contains("model.p.a") && !table.contains("model.p.c"),
            "out-of-scope rows must not appear: {table}"
        );
    }
}

// ---------------------------------------------------------------------------
// Parse-time execution
// ---------------------------------------------------------------------------

/// What running the project's parse-time checks established.
#[derive(Debug, Default)]
pub struct CheckRunOutcome {
    /// One row per check, for `run_results.json` — which is what `dbt retry` reads.
    pub results: Vec<CheckResult>,
    /// Checks that failed at `error` severity. Non-empty means the invocation must not proceed.
    pub failed: usize,
}

/// A single check's verdict, in the vocabulary `run_results` uses for assertions.
#[derive(Debug)]
pub struct CheckResult {
    pub unique_id: String,
    pub name: String,
    /// `pass` | `fail` | `warn` | `error` | `skipped`.
    pub status: &'static str,
    /// The check's configured severity: `error` | `warn`.
    pub severity: &'static str,
    /// Why the check was skipped or could not be evaluated, for telemetry. `None` for `pass` |
    /// `fail` | `warn`.
    pub reason: Option<&'static str>,
    /// How the node selection bore on this check's rows, for telemetry: `none` | `applied` |
    /// `ignored` | `unknown`.
    pub selection: &'static str,
    pub message: Option<String>,
    pub violations: Option<u64>,
    /// Wall time for this check's query and evaluation, so the result line can
    /// report it the way a test's does. Zero for a check that never ran.
    pub execution_time: f64,
    /// Violation rows rendered as a table, for the log only. `message` keeps the
    /// compact form, which is what `run_results.json` carries.
    pub rows: Option<String>,
}

impl CheckResult {
    /// A check that could not be evaluated at all — the index was missing/stale, or the check has
    /// no rendered SQL to run. Never `pass`/`fail`/`warn`: we do not know what the check would have
    /// found, so reporting anything but `error` would be a guess.
    fn error(
        unique_id: String,
        name: String,
        severity: &dbt_schemas::schemas::common::Severity,
        error: CheckError,
        selection_active: bool,
    ) -> Self {
        Self {
            unique_id,
            name,
            status: "error",
            severity: severity_str(severity),
            reason: Some(error.reason),
            selection: selection_label(selection_active, None),
            message: Some(error.message),
            violations: None,
            execution_time: 0.0,
            rows: None,
        }
    }
}

/// How the node selection bore on a check's rows. `scoped` is `None` for a check that never
/// produced rows to scope, so whether the selection would have applied is `unknown`.
fn selection_label(selection_active: bool, scoped: Option<bool>) -> &'static str {
    match (selection_active, scoped) {
        (false, _) => "none",
        (true, Some(true)) => "applied",
        (true, Some(false)) => "ignored",
        (true, None) => "unknown",
    }
}

/// Results for checks that did not run at all (`--skip-checks`, or no metadata to read), so
/// telemetry can still report them as `skipped`. They are not logged or written to
/// `run_results.json`.
pub fn not_run_results(
    checks: &[Arc<dbt_schemas::schemas::DbtCheck>],
    reason: &'static str,
    selection_active: bool,
) -> Vec<CheckResult> {
    checks
        .iter()
        .map(|c| CheckResult {
            unique_id: c.__common_attr__.unique_id.clone(),
            name: c.__common_attr__.name.clone(),
            status: "skipped",
            severity: severity_str(&severity_of(c)),
            reason: Some(reason),
            selection: selection_label(selection_active, None),
            message: None,
            violations: None,
            execution_time: 0.0,
            rows: None,
        })
        .collect()
}

/// A check's configured severity, defaulting to `error` when unset.
fn severity_of(c: &dbt_schemas::schemas::DbtCheck) -> dbt_schemas::schemas::common::Severity {
    c.deprecated_config
        .severity
        .clone()
        .unwrap_or(dbt_schemas::schemas::common::Severity::Error)
}

fn severity_str(severity: &dbt_schemas::schemas::common::Severity) -> &'static str {
    match severity {
        dbt_schemas::schemas::common::Severity::Error => "error",
        dbt_schemas::schemas::common::Severity::Warn => "warn",
    }
}

/// Violation rows as `dbt show` prints a query's rows: the same renderer, so a
/// wide result drops columns and reports the width it needs instead of wrapping
/// past the terminal.
///
/// `total` is the violation count, not the number of rows in `batch` -- the batch
/// is capped at the preview limit, and the width footer would otherwise report
/// the cap as if it were the result size.
///
/// No title. The footer appears only when rows were cut and `hint_limit` is set, so a
/// reader who sees fewer rows than the log line counts is told how to see the rest.
/// Callers whose command does not take `--limit` leave it off.
fn render_rows(
    batch: &arrow::record_batch::RecordBatch,
    total: u64,
    limit: Option<usize>,
    hint_limit: bool,
) -> Option<String> {
    let truncated = hint_limit && limit.is_some_and(|l| total > l as u64);
    let names = dbt_pretty_table::make_column_names(batch.schema_ref());
    dbt_pretty_table::pretty_data_table(
        "",
        "",
        &names,
        std::slice::from_ref(batch),
        dbt_pretty_table::DisplayFormat::Table,
        // The batch is already capped; the renderer prints its truncation footer
        // whenever it is given a limit, so give it one only when rows were cut.
        limit.filter(|_| truncated),
        false,
        Some(total as usize),
    )
    .ok()
    .map(|t| t.trim_end().to_string())
}

/// Run every check against the project metadata, before anything downstream is built.
///
/// This is the *only* place checks execute. `dbt check` and `dbt build` both call it, so the two
/// cannot drift — earlier designs ran them from two places and silently disagreed about scoping and
/// about what counted as a vacuous pass.
///
/// Placement is the whole design: parse has finished, its metadata epochs are on disk, and the task
/// graph has not been built. So a failing check needs no graph edges to stop
/// anything — the caller simply does not proceed. That removes the gating apparatus entirely, along
/// with its sharpest edge: a node configured `static_analysis: off` has no analyze task, so gating
/// only the analyze heads used to let it materialize straight through a failing check.
///
/// `selection` scopes each check's *rows* (by `unique_id`, or the columns named in `selection_filter_on`); it
/// never decides whether a check runs. A selector that matches nothing therefore yields `skipped`
/// rather than `pass`, because a green result must not stand in for a check that examined nothing.
pub fn run_parse_time_checks(
    checks: &[Arc<dbt_schemas::schemas::DbtCheck>],
    metadata_dir: &Path,
    selection: Option<&BTreeSet<String>>,
    row_limit: Option<usize>,
    hint_limit: bool,
) -> CheckRunOutcome {
    let mut outcome = CheckRunOutcome::default();
    if checks.is_empty() {
        return outcome;
    }
    let selection_active = selection.is_some();

    // Checks read the metadata this invocation just parsed. With none there is nothing to query
    // — and every check must say so rather than return zero rows, which would read as a pass.
    if let Some(reason) = metadata_unavailable_reason(metadata_dir) {
        for c in checks {
            outcome.failed += 1;
            outcome.results.push(CheckResult::error(
                c.__common_attr__.unique_id.clone(),
                c.__common_attr__.name.clone(),
                &severity_of(c),
                CheckError {
                    reason: "metadata_unavailable",
                    message: reason.clone(),
                },
                selection_active,
            ));
        }
        return outcome;
    }

    let adapter = match crate::check_adapter::open_metadata_adapter(
        metadata_dir,
        dbt_common::cancellation::never_cancels(),
    ) {
        Ok(a) => a,
        Err(e) => {
            for c in checks {
                outcome.failed += 1;
                outcome.results.push(CheckResult::error(
                    c.__common_attr__.unique_id.clone(),
                    c.__common_attr__.name.clone(),
                    &severity_of(c),
                    CheckError {
                        reason: "adapter_failed",
                        message: e.clone(),
                    },
                    selection_active,
                ));
            }
            return outcome;
        }
    };

    for c in checks {
        let name = c.__common_attr__.name.clone();
        let unique_id = c.__common_attr__.unique_id.clone();
        let severity = severity_of(c);
        let filter = selection_filter_for(c.deprecated_config.selection_filter_on.as_ref());

        let Some(sql) = c.__check_attr__.compiled_sql.as_deref() else {
            outcome.failed += 1;
            outcome.results.push(CheckResult::error(
                unique_id,
                name,
                &severity,
                CheckError {
                    reason: "no_compiled_sql",
                    message: "check has no rendered SQL".to_string(),
                },
                selection_active,
            ));
            continue;
        };

        let started = std::time::Instant::now();
        let evaluated = crate::check_adapter::query_index(&adapter, sql)
            .map_err(|message| CheckError {
                reason: "query_failed",
                message,
            })
            .and_then(|batches| {
                evaluate_batches(
                    &batches,
                    &filter,
                    selection,
                    row_limit.unwrap_or(usize::MAX),
                )
            });
        let execution_time = started.elapsed().as_secs_f64();

        match evaluated {
            // An execution error is hard whatever the severity: we do not know whether the check
            // would have passed, so reporting a pass would be a lie.
            Err(error) => {
                outcome.failed += 1;
                let mut r = CheckResult::error(unique_id, name, &severity, error, selection_active);
                r.execution_time = execution_time;
                outcome.results.push(r);
            }
            Ok(eval)
                if eval.violations == 0
                    && zero_rows_is_vacuous(selection, &filter, &eval.reported_kinds) =>
            {
                outcome.results.push(CheckResult {
                    unique_id,
                    name,
                    status: "skipped",
                    severity: severity_str(&severity),
                    reason: Some("empty_selection"),
                    selection: selection_label(selection_active, Some(eval.scoped)),
                    message: Some(
                        "selection matched no nodes this check can report on".to_string(),
                    ),
                    violations: None,
                    execution_time,
                    rows: None,
                });
            }
            Ok(eval) if eval.violations == 0 => outcome.results.push(CheckResult {
                unique_id,
                name,
                status: "pass",
                severity: severity_str(&severity),
                reason: None,
                selection: selection_label(selection_active, Some(eval.scoped)),
                message: None,
                violations: Some(0),
                execution_time,
                rows: None,
            }),
            Ok(eval) => {
                let (violations, preview) = (eval.violations, eval.preview);
                let detail = if preview.is_empty() {
                    None
                } else {
                    Some(preview.join("\n  "))
                };
                let rows = eval
                    .preview_rows
                    .as_ref()
                    .and_then(|b| render_rows(b, violations, row_limit, hint_limit));
                let hard = severity == dbt_schemas::schemas::common::Severity::Error;
                if hard {
                    outcome.failed += 1;
                }
                outcome.results.push(CheckResult {
                    unique_id,
                    name,
                    status: if hard { "fail" } else { "warn" },
                    severity: severity_str(&severity),
                    reason: None,
                    selection: selection_label(selection_active, Some(eval.scoped)),
                    message: detail,
                    violations: Some(violations),
                    execution_time,
                    rows,
                });
            }
        }
    }
    outcome
}
