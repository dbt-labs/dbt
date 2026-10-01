use glob::Pattern;

use dbt_common::{ErrorCode, FsResult, fs_err};

use super::types::{StateExplainOptions, StateExplainRecord};

pub(super) fn filter_explain_records(
    records: Vec<StateExplainRecord>,
    options: &StateExplainOptions,
) -> FsResult<Vec<StateExplainRecord>> {
    validate_selectors("select", options.select.as_deref())?;
    validate_selectors("exclude", options.exclude.as_deref())?;

    let selected = if let Some(selectors) = options
        .select
        .as_deref()
        .filter(|selectors| !selectors.is_empty())
    {
        records
            .into_iter()
            .filter(|record| {
                selectors
                    .iter()
                    .any(|selector| record_matches_selector(record, selector))
            })
            .collect()
    } else {
        records
    };

    let Some(excludes) = options
        .exclude
        .as_deref()
        .filter(|excludes| !excludes.is_empty())
    else {
        return Ok(selected);
    };

    Ok(selected
        .into_iter()
        .filter(|record| {
            !excludes
                .iter()
                .any(|exclude| record_matches_selector(record, exclude))
        })
        .collect())
}

/// Order records for display.
///
/// Nodes are appended to the explain log as they are submitted, so log order
/// depends on execution timing and varies between runs of the same project.
/// Sorting by unique id keeps rendered output stable and diffable.
pub(super) fn sort_explain_records(records: &mut [StateExplainRecord]) {
    records.sort_by(|left, right| left.node_unique_id.cmp(&right.node_unique_id));
}

fn validate_selectors(kind: &str, selectors: Option<&[String]>) -> FsResult<()> {
    let Some(selectors) = selectors else {
        return Ok(());
    };
    for selector in selectors {
        if unsupported_selector_method(selector).is_some() {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "`dbt state explain` does not support {} selector '{}'. Use plain node names, unique ids, fqn: selectors, or glob patterns.",
                kind,
                selector
            ));
        }
        warn_on_invalid_glob(kind, selector);
    }
    Ok(())
}

fn warn_on_invalid_glob(kind: &str, selector: &str) {
    let pattern = normalize_selector(selector);
    if !is_glob(pattern) {
        return;
    }
    if let Err(err) = Pattern::new(pattern) {
        tracing::warn!("Invalid {kind} glob pattern '{selector}' matches nothing: {err}");
    }
}

fn record_matches_selector(record: &StateExplainRecord, selector: &str) -> bool {
    let selector = normalize_selector(selector);

    selector_matches(selector, &record.node_unique_id)
        || record
            .node_unique_id
            .split_once('.')
            .is_some_and(|(_, fqn)| selector_matches(selector, fqn))
        || record
            .node_unique_id
            .rsplit('.')
            .next()
            .is_some_and(|name| selector_matches(selector, name))
}

fn normalize_selector(selector: &str) -> &str {
    if let Some((method, value)) = selector.split_once(':') {
        if method == "fqn" {
            return value;
        }
    }
    selector
}

fn unsupported_selector_method(selector: &str) -> Option<&str> {
    selector
        .split_once(':')
        .map(|(method, _)| method)
        .filter(|method| *method != "fqn")
}

fn selector_matches(selector: &str, text: &str) -> bool {
    if !is_glob(selector) {
        return selector == text;
    }
    Pattern::new(selector).is_ok_and(|pattern| pattern.matches(text))
}

fn is_glob(selector: &str) -> bool {
    selector.contains(&['*', '?', '[', ']'][..])
}
