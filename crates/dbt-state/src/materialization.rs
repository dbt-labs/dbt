//! Materialization classification for dbt State requests.
//!
//! Every dbt State client must classify a node's materialization the same way,
//! or the same node lands under a different cache key depending on which client
//! built the request. These predicates are the canonical classification and
//! decide three things:
//!
//! * whether a node is submitted to the service at all ([`is_table`] /
//!   [`is_view`]),
//! * whether it is submitted as `VIEW`,
//! * whether it is submitted as `DBT_CUSTOM`.
//!
//! They intentionally take the materialization as a *string* rather than a
//! `DbtMaterialization`: the classification is by name, and the enum does not
//! have a variant for every materialization name dbt accepts (`semantic_view`
//! parses as `Unknown`). Matching on strings keeps the classification correct
//! for names that have no dedicated variant.

/// Materializations dbt itself defines. Anything else is a user-defined
/// materialization macro.
///
/// `seed` is deliberately absent: seeds never reach the SQL execution-type
/// derivation, they take the `SubmitValues` path instead.
const KNOWN_MATERIALIZATIONS: &[&str] = &[
    "table",
    "view",
    "materialized_view",
    "incremental",
    "ephemeral",
    "semantic_view",
    "snapshot",
];

/// dbt's built-in incremental strategies. Any other strategy is a user-defined
/// custom strategy backed by a `get_incremental_<strategy>_sql` macro.
const KNOWN_INCREMENTAL_STRATEGIES: &[&str] = &[
    "append",
    "delete_insert",
    "merge",
    "insert_overwrite",
    "microbatch",
];

/// dbt treats a model with no `materialized` config as a view, so an empty
/// materialization normalizes to `view`.
fn or_view(materialization: &str) -> &str {
    if materialization.is_empty() {
        "view"
    } else {
        materialization
    }
}

/// Normalizes an incremental strategy name: dbt config keys may carry a `+`
/// prefix, and comparison is case-insensitive.
pub fn normalize_incremental_strategy(strategy: &str) -> String {
    strategy.replace('+', "_").to_ascii_lowercase()
}

/// Whether the node materializes as a view, i.e. a relation whose query is
/// re-evaluated on read rather than stored.
///
/// Such nodes skip dependency traversal entirely: only the view's own
/// `last_modified_epoch` and query hash matter.
pub fn is_view(materialization: &str) -> bool {
    matches!(or_view(materialization), "view" | "materialized_view")
}

/// Whether the node materializes as stored data whose freshness is worth
/// tracking.
///
/// This is a "not a view and not virtual" test rather than an allow-list, so
/// adapter-specific table-like materializations (`dynamic_table`,
/// `streaming_table`, `interactive_table`, `metric_view`, `external`, ...) are
/// tables here. They are submitted, as `DBT_CUSTOM`, because
/// [`is_custom_by_name`] rejects their names.
pub fn is_table(materialization: &str) -> bool {
    !matches!(
        or_view(materialization),
        "view" | "materialized_view" | "ephemeral" | "semantic_view"
    )
}

/// Whether the node is submitted to the dbt State service at all.
///
/// A node is submitted iff its materialization is table-like or view-like,
/// which rejects exactly `ephemeral` and `semantic_view` — neither has a
/// relation whose reuse the service can reason about.
pub fn is_submittable(materialization: &str) -> bool {
    is_table(materialization) || is_view(materialization)
}

/// Whether an incremental node uses a user-defined incremental strategy.
///
/// A missing strategy means dbt falls back to the adapter's default, which is
/// always one of the built-in strategies.
pub fn is_custom_incremental_strategy(
    materialization: &str,
    incremental_strategy: Option<&str>,
) -> bool {
    if materialization != "incremental" {
        return false;
    }
    incremental_strategy.is_some_and(|strategy| {
        !KNOWN_INCREMENTAL_STRATEGIES.contains(&normalize_incremental_strategy(strategy).as_str())
    })
}

/// Whether the node runs a user-defined materialization, by *name*.
///
/// A custom materialization that *shadows* a built-in name is not detectable
/// here; that requires resolving which macro dbt would actually dispatch, and
/// is applied on top of this check by the caller (dbt-core#14486).
///
/// An empty materialization is not custom: a node with no `materialized` config
/// runs dbt's default view materialization, which is checked before the `view`
/// defaulting that [`is_view`] and [`is_table`] apply.
pub fn is_custom_by_name(materialization: &str, incremental_strategy: Option<&str>) -> bool {
    if materialization.is_empty() {
        return false;
    }
    if !KNOWN_MATERIALIZATIONS.contains(&materialization) {
        return true;
    }
    is_custom_incremental_strategy(materialization, incremental_strategy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_view_and_materialized_view_are_views() {
        assert!(is_view("view"));
        assert!(is_view("materialized_view"));
        // dbt defaults an unset materialization to view.
        assert!(is_view(""));

        assert!(!is_view("table"));
        assert!(!is_view("incremental"));
        assert!(!is_view("snapshot"));
        assert!(!is_view("ephemeral"));
        assert!(!is_view("semantic_view"));
        assert!(!is_view("metric_view"));
        assert!(!is_view("dynamic_table"));
    }

    #[test]
    fn everything_but_views_and_virtual_relations_is_a_table() {
        assert!(is_table("table"));
        assert!(is_table("incremental"));
        assert!(is_table("snapshot"));
        assert!(is_table("seed"));
        // Adapter-specific table-likes are tables, and therefore submitted.
        assert!(is_table("dynamic_table"));
        assert!(is_table("streaming_table"));
        assert!(is_table("interactive_table"));
        assert!(is_table("metric_view"));
        assert!(is_table("external"));
        assert!(is_table("some_user_materialization"));

        assert!(!is_table("view"));
        assert!(!is_table(""));
        assert!(!is_table("materialized_view"));
        assert!(!is_table("ephemeral"));
        assert!(!is_table("semantic_view"));
    }

    #[test]
    fn only_ephemeral_and_semantic_view_are_not_submitted() {
        assert!(!is_submittable("ephemeral"));
        assert!(!is_submittable("semantic_view"));

        for materialization in [
            "view",
            "",
            "materialized_view",
            "table",
            "incremental",
            "snapshot",
            "seed",
            "dynamic_table",
            "metric_view",
            "streaming_table",
        ] {
            assert!(
                is_submittable(materialization),
                "{materialization} should be submitted"
            );
        }
    }

    #[test]
    fn materializations_outside_dbts_own_set_are_custom() {
        // Adapter-specific materializations are not in dbt's known set.
        assert!(is_custom_by_name("dynamic_table", None));
        assert!(is_custom_by_name("metric_view", None));
        assert!(is_custom_by_name("streaming_table", None));
        assert!(is_custom_by_name("interactive_table", None));
        assert!(is_custom_by_name("external", None));
        assert!(is_custom_by_name("my_materialization", None));

        assert!(!is_custom_by_name("table", None));
        assert!(!is_custom_by_name("view", None));
        assert!(!is_custom_by_name("materialized_view", None));
        assert!(!is_custom_by_name("incremental", None));
        assert!(!is_custom_by_name("snapshot", None));
        assert!(!is_custom_by_name("ephemeral", None));
        assert!(!is_custom_by_name("semantic_view", None));
        // A node with no materialization runs dbt's default view materialization.
        assert!(!is_custom_by_name("", None));
    }

    #[test]
    fn custom_incremental_strategy_makes_the_node_custom() {
        assert!(is_custom_by_name("incremental", Some("my_strategy")));
        assert!(!is_custom_by_name("incremental", Some("merge")));
        assert!(!is_custom_by_name("incremental", Some("microbatch")));
        // dbt config keys use `+` prefixes that normalize to `_`.
        assert!(!is_custom_by_name("incremental", Some("insert+overwrite")));
        assert!(!is_custom_by_name("incremental", Some("MERGE")));
        // A strategy on a non-incremental node is ignored.
        assert!(!is_custom_by_name("table", Some("my_strategy")));
    }
}
