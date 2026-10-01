//! Testing utilities for relation configs
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigChange, RelationComponentConfigChangeSet, RelationConfig,
    RelationConfigLoader,
};
use dbt_schemas::schemas::nodes::DbtModel;
use minijinja_contrib::testing::jinja_diff;

const JINJA_INTROSPECT: &str = "
{% macro introspect(obj) %}
    {% if obj is number or obj is string or obj is boolean or obj is none or obj is undefined %}
        {{ obj }}
    {% elif obj is mapping %}
        {% for key in obj %}
            <{{ key }}>
            {{ introspect(obj[key]) }}
            </{{ key }}>
        {% endfor %}
    {% elif obj is iterable %}
        {% for val in obj %}
            {{ introspect(val) }}
        {% endfor %}
    {% else %}
        UNKNOWN
    {% endif %}
{% endmacro %}

{{ introspect(obj) }}
";

/// A single test case to test whether a relation config
/// of a given type results in the correct changeset
pub(crate) struct TestCase<L, C> {
    pub description: &'static str,
    pub current_state: C,
    pub desired_state: C,
    pub relation_loader: RelationConfigLoader<'static, L>,
    pub expected_changeset: RelationComponentConfigChangeSet,
    pub changeset_jinja: &'static str,
    pub requires_full_refresh: bool,
}

fn components_eq(a: &dyn ComponentConfig, b: &dyn ComponentConfig) -> bool {
    let a_diff = a.diff_from(Some(b));
    let b_diff = b.diff_from(Some(a));

    a_diff.is_none() && b_diff.is_none()
}

fn changesets_eq(
    a: &RelationComponentConfigChangeSet,
    b: &RelationComponentConfigChangeSet,
) -> Vec<String> {
    if a.adapter_type() != b.adapter_type() {
        return vec![format!(
            "adapter_type mismatch (expected {}, got {})",
            a.adapter_type(),
            b.adapter_type()
        )];
    }

    if a.len() != b.len() {
        return vec![format!(
            "changeset lengths differ (expected {}, got {})",
            a.len(),
            b.len()
        )];
    }

    let mut errors = Vec::new();

    for (a_type, a_change) in a.iter() {
        let b_change = b.get(a_type);

        let err = match (a_change, b_change) {
            (
                ComponentConfigChange::Some(a_component),
                ComponentConfigChange::Some(b_component),
            ) => {
                if !components_eq(a_component.as_ref(), b_component.as_ref()) {
                    Some(format!(
                        "diffs mismatch \n     expected: {a_component:?}\n     got     : {b_component:?}"
                    ))
                } else {
                    None
                }
            }
            (ComponentConfigChange::Drop, ComponentConfigChange::Drop) => None,
            (ComponentConfigChange::None, ComponentConfigChange::None) => None,
            (a, b) => Some(format!("config change types mismatch ({a:?}, {b:?})")),
        };

        if let Some(err) = err {
            errors.push(format!("{a_type} :: {err}"));
        }
    }

    debug_assert!(
        !errors.is_empty() || a.requires_full_refresh() == b.requires_full_refresh(),
        "Programming error, this is a bug"
    );

    errors
}

pub(crate) fn run_test_case<L, C>(
    tc: TestCase<L, C>,
    create_mock_local_config: fn(C) -> DbtModel,
) -> Option<String> {
    let desired_local_config = create_mock_local_config(tc.desired_state);
    let current_local_config = create_mock_local_config(tc.current_state);

    let desired = tc
        .relation_loader
        .from_local_config(&desired_local_config)
        .expect("desired config loader failed: test setup produced an invalid model config");
    let current = tc
        .relation_loader
        .from_local_config(&current_local_config)
        .expect("current config loader failed: test setup produced an invalid model config");
    let changeset = RelationConfig::diff(&desired, &current);

    let mut errors = changesets_eq(&tc.expected_changeset, &changeset);

    if changeset.requires_full_refresh() != tc.requires_full_refresh {
        errors.push(format!(
            "expected requires_full_refresh={}\n",
            tc.requires_full_refresh
        ));
    }

    if !errors.is_empty() {
        let mut out = String::new();
        out.push_str(tc.description);

        errors.sort();

        out.push_str("\nOVERALL ERRORS\n");
        for err in errors {
            out.push_str("  > ");
            out.push_str(err.as_str());
            out.push('\n');
        }

        out.push_str("EXPECTED CHANGESET\n");
        out.push_str(format!("{:#?}", tc.expected_changeset).as_str());

        out.push_str("\n\nGOT CHANGESET\n");
        out.push_str(format!("{:#?}", changeset).as_str());

        Some(out)
    } else if let Some(diff) = jinja_diff(changeset, JINJA_INTROSPECT, tc.changeset_jinja) {
        let mut out = String::with_capacity(diff.len() + 50);
        out.push_str("JINJA MISMATCH ERROR\n");
        out.push_str(&diff);
        Some(out)
    } else {
        None
    }
}

/// Run all test cases. Panic if any of them fails.
pub(crate) fn run_test_cases<L, C>(
    test_cases: Vec<TestCase<L, C>>,
    create_mock_local_config: fn(C) -> DbtModel,
) {
    let mut errors = Vec::new();
    for tc in test_cases {
        if let Some(err) = run_test_case(tc, create_mock_local_config) {
            errors.push(err);
        }
    }

    if !errors.is_empty() {
        let err_str = errors.join("\n>>>>> ");
        panic!(">>>>> {err_str}");
    }
}
