//! Replacing the computed node selection with an externally supplied node set.
//!
//! When active, the supplied set of node `unique_id`s takes the place of everything the selection
//! inputs (`--select`, `--exclude`, `--resource-type`, `--exclude-resource-type`) would have
//! produced. Whoever supplies the set owns whether that is appropriate; the engine just honors it.
//!
//! Two env vars drive it, and only inside a Mantle replay:
//!
//! - [`MANTLE_ARTIFACTS_ENV`] — a directory. Location only; on its own it changes nothing.
//! - [`OVERRIDE_FROM_RUN_RESULTS_ENV`] — boolean. Turns that directory's `run_results.json` into
//!   the selection.
//!
//! The seam is narrow: the scheduler core takes only a resolved [`SelectionOverride`], and one
//! function ([`load_selection_override`]) spans directory-to-set. Changing the input format means
//! replacing that one body.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dbt_common::io_args::{EvalArgs, FsCommand, ReplayMode, env_flag_enabled, env_path};
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use serde::Deserialize;

use crate::schemas::Nodes;
use crate::schemas::common::DbtMaterialization;
use crate::schemas::telemetry::NodeType;

/// The one file name read from the supplied directory. There is no search and no alternate name.
pub const SELECTION_OVERRIDE_FILE_NAME: &str = "run_results.json";

/// `unique_id` prefixes that are never schedulable nodes, and so are dropped from a supplied set.
///
/// A run's reported results are not a clean node set. Hook operations in particular do have real
/// entries in [`Nodes`], so an `operation.*` id would otherwise reach the scheduler as a selected
/// node.
const NON_NODE_ID_PREFIXES: &[&str] = &["operation.", "exposure."];

/// How many ids a divergence message names before it stops enumerating.
pub const SAMPLE_CAP: usize = 10;

/// An externally supplied set of node `unique_id`s that replaces the computed selection.
///
/// Constructed only by [`load_selection_override`] or, in tests, [`SelectionOverride::from_ids`],
/// so that everything downstream can rely on the set having been filtered and guarded once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionOverride {
    ids: BTreeSet<String>,
    source: PathBuf,
}

impl SelectionOverride {
    /// The supplied node ids.
    pub fn ids(&self) -> &BTreeSet<String> {
        &self.ids
    }

    /// The file the ids came from, for messages.
    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Build an override directly from ids, bypassing the on-disk format.
    pub fn from_ids(ids: impl IntoIterator<Item = String>, source: impl Into<PathBuf>) -> Self {
        Self {
            ids: ids.into_iter().filter(|id| is_node_id(id)).collect(),
            source: source.into(),
        }
    }
}

fn is_node_id(unique_id: &str) -> bool {
    !NON_NODE_ID_PREFIXES
        .iter()
        .any(|prefix| unique_id.starts_with(prefix))
}

/// The two fields this module reads out of a prior run's results.
///
/// Deliberately minimal and lenient rather than the full artifact schema: a producer that shifts
/// any of the fields we never read must not fail the load. Serde ignores unknown fields by default,
/// which is the wanted behavior here. A useful consequence is that *producing* an override is also
/// trivial — a document holding nothing but `{"results":[{"unique_id":"..."}]}` is valid input.
#[derive(Deserialize)]
struct RunResultsSelectionView {
    results: Vec<RunResultSelectionRow>,
}

#[derive(Deserialize)]
struct RunResultSelectionRow {
    unique_id: String,
}

/// Resolve an externally supplied set of node ids from a prior run's artifacts.
///
/// The single seam between "a directory on disk" and the resolved set: changing the input format
/// means replacing this function body. The lookup is exactly
/// `artifacts_dir/`[`SELECTION_OVERRIDE_FILE_NAME`] — no search, no format sniffing, no mode
/// argument.
///
/// Every failure is loud, and none degrades silently to the computed selection: a missing or
/// malformed file is an error naming the path. An empty `results` array is *not* a failure — it
/// resolves to an empty override, which is a legitimate instruction to run no nodes and is a
/// materially different outcome from not overriding at all.
pub fn load_selection_override(artifacts_dir: &Path) -> FsResult<SelectionOverride> {
    let path = artifacts_dir.join(SELECTION_OVERRIDE_FILE_NAME);

    let contents = std::fs::read_to_string(&path).map_err(|e| {
        fs_err!(
            ErrorCode::FileNotFound,
            "Cannot read the node set that should replace the computed selection: failed to read '{}': {e}",
            path.display()
        )
    })?;

    parse_selection_override(&contents, path)
}

/// Directory holding a prior run's artifacts. Location only — setting it enables nothing.
pub const MANTLE_ARTIFACTS_ENV: &str = "DBT_ENGINE_MANTLE_ARTIFACTS";

/// Turns [`MANTLE_ARTIFACTS_ENV`]'s `run_results.json` into this run's selection.
pub const OVERRIDE_FROM_RUN_RESULTS_ENV: &str = "DBT_ENGINE_OVERRIDE_SELECTION_FROM_RUN_RESULTS";

/// Derives the selection from the replay recording instead. Consumed by the conformance tooling,
/// which synthesizes a `run_results.json` and sets the two vars above; the engine only rejects it
/// in combination with [`OVERRIDE_FROM_RUN_RESULTS_ENV`], so the two sources cannot both claim to
/// be authoritative.
pub const OVERRIDE_FROM_RECORDING_ENV: &str = "DBT_ENGINE_OVERRIDE_SELECTION_FROM_RECORDING";

/// Whether an externally supplied node set may apply to this invocation at all.
///
/// Gated on Mantle replay, which is the only context this exists for — so the env vars are inert in
/// a normal run however they were set. Freshness-shaped commands are excluded because they write
/// `sources.json`, so there is no node set to read.
pub fn selection_override_applies(arg: &EvalArgs) -> bool {
    matches!(&arg.replay, Some(ReplayMode::MantleReplay(_))) && arg.command != FsCommand::Source
}

/// Resolve the node set that should replace the computed selection, if any.
///
/// Reads the environment rather than taking a flag, so nothing has to be threaded through
/// `EvalArgs`. Every failure is loud: the request is explicit, and falling back to the computed
/// selection would produce a green-looking replay that proves nothing.
pub fn resolve_selection_override(arg: &EvalArgs) -> FsResult<Option<SelectionOverride>> {
    let from_run_results = env_flag_enabled(OVERRIDE_FROM_RUN_RESULTS_ENV)?;
    let from_recording = env_flag_enabled(OVERRIDE_FROM_RECORDING_ENV)?;

    if from_run_results && from_recording {
        return err!(
            ErrorCode::InvalidConfig,
            "{OVERRIDE_FROM_RUN_RESULTS_ENV} and {OVERRIDE_FROM_RECORDING_ENV} are both set; \
             they name different sources for the same node set, so pick one"
        );
    }

    if !from_run_results || !selection_override_applies(arg) {
        return Ok(None);
    }

    let Some(dir) = env_path(MANTLE_ARTIFACTS_ENV) else {
        return err!(
            ErrorCode::MissingArgument,
            "{OVERRIDE_FROM_RUN_RESULTS_ENV} is set but {MANTLE_ARTIFACTS_ENV} is not, so there is \
             no '{SELECTION_OVERRIDE_FILE_NAME}' to read the node set from"
        );
    };

    Ok(Some(load_selection_override(&dir)?))
}

/// The parsing half of [`load_selection_override`], split out so it can be exercised without a
/// filesystem. `path` is carried only for messages and for [`SelectionOverride::source`].
pub fn parse_selection_override(
    contents: &str,
    path: impl Into<PathBuf>,
) -> FsResult<SelectionOverride> {
    let path = path.into();

    let view: RunResultsSelectionView = serde_json::from_str(contents).map_err(|e| {
        fs_err!(
            ErrorCode::JsonInvalid,
            "Cannot read the node set that should replace the computed selection: '{}' is not a document with a 'results' array of objects carrying 'unique_id': {e}",
            path.display()
        )
    })?;

    Ok(SelectionOverride {
        ids: view
            .results
            .into_iter()
            .map(|row| row.unique_id)
            .filter(|id| is_node_id(id))
            .collect(),
        source: path,
    })
}

/// Resource types a run never reports results for.
///
/// These are not runnable, so they cannot appear in a supplied ran-set, and counting them on the
/// computed side would put `fusion_would_select` systematically above `injected` for reasons that
/// have nothing to do with selection.
pub fn is_reportable_resource_type(node_type: NodeType) -> bool {
    !matches!(
        node_type,
        NodeType::Exposure
            | NodeType::Analysis
            | NodeType::SemanticModel
            | NodeType::Metric
            | NodeType::SavedQuery
            | NodeType::Operation
            | NodeType::Macro
            | NodeType::DocsMacro
            | NodeType::Unspecified
    )
}

/// Retain only the ids whose resource type a run would report on.
pub fn retain_reportable(ids: &BTreeSet<String>, nodes: &Nodes) -> BTreeSet<String> {
    ids.iter()
        .filter(|id| {
            nodes
                .get_node(id)
                .is_none_or(|node| is_reportable_resource_type(node.resource_type()))
        })
        .cloned()
        .collect()
}

/// How far the supplied node set and the selection this engine would have computed diverge.
///
/// Purely descriptive: nothing here changes what runs. The counters are the measurement the
/// override exists to make possible, since an override that silently succeeded would take the
/// magnitude of the divergence with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionOverrideStats {
    /// Ids supplied, after non-node ids were dropped.
    pub injected: usize,
    /// Supplied ids that resolve to a node this engine knows about.
    pub matched: usize,
    /// Supplied ids with no counterpart node here.
    pub unmatched: usize,
    /// Size of the selection this engine computed from its own selection inputs, counting only
    /// resource types a run reports on.
    pub fusion_would_select: usize,
    /// Ids this engine would have selected that were not supplied (over-selection).
    pub fusion_only: usize,
    /// Matched ids that the schedulability filters removed.
    pub dropped_unschedulable: usize,
    /// Up to [`SAMPLE_CAP`] unmatched ids, so a message can be specific without spewing.
    pub unmatched_sample: Vec<String>,
    /// Up to [`SAMPLE_CAP`] dropped ids.
    pub dropped_unschedulable_sample: Vec<String>,
}

/// Compare a supplied node set against the selection this engine computed for the same run.
///
/// * `injected` — the supplied ids
/// * `computed` — the selection the engine's own selection inputs produced
/// * `matched` — the supplied ids that resolved to a node
/// * `scheduled` — what survived the schedulability filters
pub fn compute_selection_override_stats(
    injected: &BTreeSet<String>,
    computed: &BTreeSet<String>,
    matched: &BTreeSet<String>,
    scheduled: &BTreeSet<String>,
    nodes: &Nodes,
) -> SelectionOverrideStats {
    let unmatched: Vec<String> = injected.difference(matched).cloned().collect();
    let dropped: Vec<String> = matched.difference(scheduled).cloned().collect();
    let computed_reportable = retain_reportable(computed, nodes);
    let fusion_only = computed_reportable.difference(injected).count();

    SelectionOverrideStats {
        injected: injected.len(),
        matched: matched.len(),
        unmatched: unmatched.len(),
        fusion_would_select: computed_reportable.len(),
        fusion_only,
        dropped_unschedulable: dropped.len(),
        unmatched_sample: unmatched.into_iter().take(SAMPLE_CAP).collect(),
        dropped_unschedulable_sample: dropped.into_iter().take(SAMPLE_CAP).collect(),
    }
}

/// How the nodes a run actually reported compare against the supplied node set.
///
/// This is the invariant the override targets, and the primary signal. The schedule-level counters
/// describe a different level and can look clean while this one fails: a node can be scheduled
/// correctly and then never produce a result row, silently leaving the reported set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionOverrideReconciliation {
    /// Reported ids, after exempt classes were removed.
    pub reported: usize,
    /// Supplied ids, after exempt classes were removed.
    pub injected: usize,
    /// Reported but not supplied — the engine ran more than it was told to.
    pub ran_not_injected: Vec<String>,
    /// Supplied but not reported — the engine ran less than it was told to.
    pub injected_not_ran: Vec<String>,
}

impl SelectionOverrideReconciliation {
    pub fn is_clean(&self) -> bool {
        self.ran_not_injected.is_empty() && self.injected_not_ran.is_empty()
    }
}

/// Ephemeral models are inlined as CTEs by other engines and never appear in their results, while
/// this engine reports a skipped row for each. Without exempting them, every run with an ephemeral
/// model reports divergence that has nothing to do with selection.
fn is_ephemeral_model(unique_id: &str, nodes: &Nodes) -> bool {
    nodes
        .models
        .get(unique_id)
        .is_some_and(|model| model.__base_attr__.materialized == DbtMaterialization::Ephemeral)
}

/// Diff the nodes a run reported against the node set it was told to run.
///
/// The two sides are reduced to their comparable parts first, and **not symmetrically**:
///
/// * from the reported side — sources (never executed by run-shaped commands) and ephemeral models,
///   both of which this engine may report on while the engine that produced the supplied set does
///   not;
/// * from the supplied side — the resource types a run never reports results for at all
///   ([`is_reportable_resource_type`]).
///
/// Sources are deliberately dropped from the reported side but **kept** on the supplied side. A real
/// ran-set cannot name a source, because `run`/`build` never execute one — so a supplied set that
/// does name one is malformed, and saying so is more useful than tolerating it silently. The cost of
/// the asymmetry is a `injected_not_ran` entry pointing straight at the bad id.
pub fn reconcile_reported_nodes(
    injected: &BTreeSet<String>,
    reported: &BTreeSet<String>,
    nodes: &Nodes,
) -> SelectionOverrideReconciliation {
    let reported: BTreeSet<String> = reported
        .iter()
        .filter(|id| !nodes.sources.contains_key(*id) && !is_ephemeral_model(id, nodes))
        .filter(|id| is_node_id(id))
        .cloned()
        .collect();
    let injected = retain_reportable(injected, nodes);

    SelectionOverrideReconciliation {
        ran_not_injected: reported.difference(&injected).cloned().collect(),
        injected_not_ran: injected.difference(&reported).cloned().collect(),
        reported: reported.len(),
        injected: injected.len(),
    }
}

/// Render a bounded id list for a message: `a, b, c (and 4 more)`.
pub fn format_sample(sample: &[String], total: usize) -> String {
    if sample.is_empty() {
        return "none".to_string();
    }
    let mut rendered = sample.join(", ");
    if total > sample.len() {
        rendered.push_str(&format!(" (and {} more)", total - sample.len()));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "/artifacts/run_results.json";

    fn parse(contents: &str) -> FsResult<SelectionOverride> {
        parse_selection_override(contents, SOURCE)
    }

    #[test]
    fn ids_are_projected_past_unknown_sibling_fields() {
        // The leniency claim: a producer carrying fields we never read, and omitting fields the
        // full artifact schema requires, still loads.
        let over = parse(
            r#"{
                "metadata": {"dbt_schema_version": "something-else", "extra": 1},
                "elapsed_time": 1.5,
                "results": [
                    {"unique_id": "model.pkg.a", "status": "success", "unheard_of": [1, 2]},
                    {"unique_id": "test.pkg.b", "thread_id": "Thread-19 (worker)"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            over.ids(),
            &BTreeSet::from(["model.pkg.a".to_string(), "test.pkg.b".to_string()])
        );
        assert_eq!(over.source(), Path::new(SOURCE));
    }

    #[test]
    fn empty_results_is_an_empty_override_not_an_error() {
        let over = parse(r#"{"results": []}"#).unwrap();
        assert!(over.is_empty());
        assert_eq!(over.len(), 0);
    }

    #[test]
    fn non_node_rows_are_filtered_out() {
        // Shaped like a real artifact: hook operations and exposures are reported by some
        // producers and must not reach the scheduler as selected nodes.
        let over = parse(
            r#"{
                "results": [
                    {"unique_id": "operation.swyft_dbt.swyft_dbt-on-run-end-0", "status": "success"},
                    {"unique_id": "exposure.swyft_dbt.weekly_revenue", "status": "no-op"},
                    {"unique_id": "model.swyft_dbt.orders", "status": "success"},
                    {"unique_id": "seed.swyft_dbt.countries", "status": "success"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            over.ids(),
            &BTreeSet::from([
                "model.swyft_dbt.orders".to_string(),
                "seed.swyft_dbt.countries".to_string(),
            ])
        );
    }

    #[test]
    fn malformed_json_errors_with_the_path() {
        let err = parse("{ not json").unwrap_err();
        assert!(
            err.pretty().contains(SOURCE),
            "message should name the path: {}",
            err.pretty()
        );
    }

    #[test]
    fn document_without_results_errors_with_the_path() {
        let err = parse(r#"{"metadata": {"invocation_id": "abc"}}"#).unwrap_err();
        assert!(
            err.pretty().contains(SOURCE),
            "message should name the path: {}",
            err.pretty()
        );
    }

    #[test]
    fn missing_file_errors_with_the_path() {
        let dir = Path::new("/this/directory/does/not/exist/for/selection/override");
        let err = load_selection_override(dir).unwrap_err();
        assert!(
            err.pretty()
                .contains(dir.join(SELECTION_OVERRIDE_FILE_NAME).to_str().unwrap()),
            "message should name the path: {}",
            err.pretty()
        );
    }

    #[test]
    fn from_ids_filters_non_node_ids_too() {
        let over = SelectionOverride::from_ids(
            [
                "model.pkg.a".to_string(),
                "operation.pkg.pkg-on-run-start-0".to_string(),
                "exposure.pkg.e".to_string(),
            ],
            "synthesized",
        );
        assert_eq!(over.ids(), &BTreeSet::from(["model.pkg.a".to_string()]));
    }

    fn replay_args(command: FsCommand, replay: Option<ReplayMode>) -> EvalArgs {
        EvalArgs {
            command,
            replay,
            ..Default::default()
        }
    }

    #[test]
    fn an_override_applies_only_inside_a_mantle_replay() {
        // The whole mechanism exists for replay. Outside one the env vars are inert however they
        // were set, so it cannot reach a production run.
        let replay = || Some(ReplayMode::MantleReplay("rec.json".into()));

        for command in [
            FsCommand::Build,
            FsCommand::Run,
            FsCommand::Test,
            FsCommand::List,
            FsCommand::Compile,
            FsCommand::Seed,
            FsCommand::Snapshot,
        ] {
            assert!(
                selection_override_applies(&replay_args(command, replay())),
                "{command:?} under replay should honor an override"
            );
            assert!(
                !selection_override_applies(&replay_args(command, None)),
                "{command:?} outside replay must not"
            );
        }

        // Other replay flavours are not Mantle replays.
        assert!(!selection_override_applies(&replay_args(
            FsCommand::Build,
            Some(ReplayMode::FsReplay("rec.json".into()))
        )));

        // `dbt source freshness` writes `sources.json`, so there is no node set to read for it. It
        // still builds a schedule, which is why this is a carve-out rather than moot.
        assert!(!selection_override_applies(&replay_args(
            FsCommand::Source,
            replay()
        )));
    }

    // -----------------------------------------------------------------------------------------
    // Counters and exemptions
    // -----------------------------------------------------------------------------------------

    fn ids(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    /// A model, seed, test, exposure, metric and ephemeral model, so both exemption directions
    /// have something to exempt.
    fn stats_nodes() -> Nodes {
        use crate::schemas::nodes::{DbtExposure, DbtModel, DbtSeed, DbtTest};
        use crate::schemas::{CommonAttributes, NodeBaseAttributes};
        use std::sync::Arc;

        fn common(id: &str) -> CommonAttributes {
            CommonAttributes {
                unique_id: id.to_string(),
                name: id.to_string(),
                ..Default::default()
            }
        }

        let mut nodes = Nodes::default();
        for id in ["model.pkg.a", "model.pkg.b"] {
            nodes.models.insert(
                id.to_string(),
                Arc::new(DbtModel {
                    __common_attr__: common(id),
                    ..Default::default()
                }),
            );
        }
        nodes.models.insert(
            "model.pkg.eph".to_string(),
            Arc::new(DbtModel {
                __common_attr__: common("model.pkg.eph"),
                __base_attr__: NodeBaseAttributes {
                    materialized: DbtMaterialization::Ephemeral,
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        nodes.seeds.insert(
            "seed.pkg.s".to_string(),
            Arc::new(DbtSeed {
                __common_attr__: common("seed.pkg.s"),
                ..Default::default()
            }),
        );
        nodes.tests.insert(
            "test.pkg.t".to_string(),
            Arc::new(DbtTest {
                __common_attr__: common("test.pkg.t"),
                ..Default::default()
            }),
        );
        nodes.exposures.insert(
            "exposure.pkg.e".to_string(),
            Arc::new(DbtExposure {
                __common_attr__: common("exposure.pkg.e"),
                ..Default::default()
            }),
        );
        nodes
    }

    #[test]
    fn counters_describe_both_directions_of_divergence() {
        let nodes = stats_nodes();
        let injected = ids(&["model.pkg.a", "model.pkg.gone"]);
        let computed = ids(&["model.pkg.a", "model.pkg.b", "seed.pkg.s"]);
        let matched = ids(&["model.pkg.a"]);
        let scheduled = ids(&["model.pkg.a"]);

        let stats =
            compute_selection_override_stats(&injected, &computed, &matched, &scheduled, &nodes);

        assert_eq!(stats.injected, 2);
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.unmatched, 1);
        assert_eq!(stats.unmatched_sample, vec!["model.pkg.gone".to_string()]);
        assert_eq!(stats.fusion_would_select, 3);
        // `model.pkg.a` is on both sides; the other two are over-selection.
        assert_eq!(stats.fusion_only, 2);
        assert_eq!(stats.dropped_unschedulable, 0);
    }

    #[test]
    fn dropped_unschedulable_counts_matched_ids_the_engine_cannot_run() {
        let nodes = stats_nodes();
        let injected = ids(&["model.pkg.a", "model.pkg.b"]);
        let matched = injected.clone();
        let scheduled = ids(&["model.pkg.a"]);

        let stats = compute_selection_override_stats(
            &injected,
            &BTreeSet::new(),
            &matched,
            &scheduled,
            &nodes,
        );

        assert_eq!(stats.dropped_unschedulable, 1);
        assert_eq!(
            stats.dropped_unschedulable_sample,
            vec!["model.pkg.b".to_string()]
        );
        assert_eq!(stats.unmatched, 0);
    }

    #[test]
    fn fusion_would_select_excludes_resource_types_a_run_never_reports() {
        // Otherwise this counter sits systematically above `injected` for reasons unrelated to
        // selection: `dbt ls` lists an exposure, a run's results never mention one.
        let nodes = stats_nodes();
        let computed = ids(&["model.pkg.a", "exposure.pkg.e"]);

        let stats = compute_selection_override_stats(
            &BTreeSet::new(),
            &computed,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &nodes,
        );

        assert_eq!(stats.fusion_would_select, 1);
        assert_eq!(stats.fusion_only, 1);
    }

    #[test]
    fn sample_lists_are_capped() {
        let nodes = Nodes::default();
        let injected: BTreeSet<String> = (0..SAMPLE_CAP + 5)
            .map(|i| format!("model.pkg.m{i:02}"))
            .collect();

        let stats = compute_selection_override_stats(
            &injected,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &nodes,
        );

        assert_eq!(stats.unmatched, SAMPLE_CAP + 5);
        assert_eq!(stats.unmatched_sample.len(), SAMPLE_CAP);
    }

    #[test]
    fn reconciliation_reports_both_directions() {
        let nodes = stats_nodes();
        let injected = ids(&["model.pkg.a", "model.pkg.b"]);
        let reported = ids(&["model.pkg.a", "seed.pkg.s"]);

        let report = reconcile_reported_nodes(&injected, &reported, &nodes);

        assert_eq!(report.injected, 2);
        assert_eq!(report.reported, 2);
        assert_eq!(report.ran_not_injected, vec!["seed.pkg.s".to_string()]);
        assert_eq!(report.injected_not_ran, vec!["model.pkg.b".to_string()]);
        assert!(!report.is_clean());
    }

    #[test]
    fn reconciliation_exempts_ephemeral_models_and_sources_from_ran_not_injected() {
        // This engine reports a skipped row for an ephemeral model; the engine that produced the
        // supplied set inlines them as CTEs and never lists them. Without the exemption every run
        // with an ephemeral model reports divergence unrelated to selection.
        let mut nodes = stats_nodes();
        {
            use crate::schemas::CommonAttributes;
            use crate::schemas::nodes::DbtSource;
            use std::sync::Arc;
            nodes.sources.insert(
                "source.pkg.raw.events".to_string(),
                Arc::new(DbtSource {
                    __common_attr__: CommonAttributes {
                        unique_id: "source.pkg.raw.events".to_string(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        let injected = ids(&["model.pkg.a"]);
        let reported = ids(&["model.pkg.a", "model.pkg.eph", "source.pkg.raw.events"]);

        let report = reconcile_reported_nodes(&injected, &reported, &nodes);

        assert!(report.is_clean(), "unexpected divergence: {report:?}");
        assert_eq!(report.reported, 1);
    }

    #[test]
    fn a_supplied_source_id_is_reported_as_unrun_rather_than_exempted() {
        // Asymmetric on purpose: sources are dropped from the *reported* side, but a supplied set
        // that names one is malformed — `run`/`build` never execute a source, so no real ran-set
        // contains one. Surfacing it beats tolerating it.
        //
        // This is also the case that shows why the post-run reconciliation exists at all: at
        // schedule time a source id resolves to a real node and is counted `matched`, so the
        // schedule-level counters come out perfectly clean while the invariant is broken.
        let mut nodes = stats_nodes();
        {
            use crate::schemas::CommonAttributes;
            use crate::schemas::nodes::DbtSource;
            use std::sync::Arc;
            nodes.sources.insert(
                "source.pkg.raw.events".to_string(),
                Arc::new(DbtSource {
                    __common_attr__: CommonAttributes {
                        unique_id: "source.pkg.raw.events".to_string(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        let injected = ids(&["model.pkg.a", "source.pkg.raw.events"]);
        let reported = ids(&["model.pkg.a"]);

        let report = reconcile_reported_nodes(&injected, &reported, &nodes);

        assert!(!report.is_clean());
        assert_eq!(
            report.injected_not_ran,
            vec!["source.pkg.raw.events".to_string()]
        );
        assert!(report.ran_not_injected.is_empty());
    }

    #[test]
    fn reconciliation_exempts_non_reportable_types_from_injected_not_ran() {
        // Without this, every supplied exposure and hook operation is reported as unrun forever.
        let nodes = stats_nodes();
        let injected = ids(&["model.pkg.a", "exposure.pkg.e"]);
        let reported = ids(&["model.pkg.a"]);

        let report = reconcile_reported_nodes(&injected, &reported, &nodes);

        assert!(report.is_clean(), "unexpected divergence: {report:?}");
        assert_eq!(report.injected, 1);
    }

    #[test]
    fn format_sample_caps_and_reports_the_remainder() {
        assert_eq!(format_sample(&[], 0), "none");
        assert_eq!(
            format_sample(&["a".to_string(), "b".to_string()], 2),
            "a, b"
        );
        assert_eq!(
            format_sample(&["a".to_string(), "b".to_string()], 9),
            "a, b (and 7 more)"
        );
    }
}
