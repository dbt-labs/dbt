use std::collections::HashMap;

use dbt_adapter_core::AdapterType;
use dbt_jinja_ctx::DbtNamespace;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

/// Render `template` with every macro `adapter_type` ships, lowercased and with
/// whitespace collapsed so assertions don't depend on macro formatting.
fn render(adapter_type: AdapterType, template: &str) -> String {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        // The harness does not wire package namespaces, and these macros
        // dispatch through the `dbt` namespace.
        .with_global("dbt", Value::from_object(DbtNamespace::new("dbt")))
        .build()
        .expect("harness should build");

    harness
        .render(template, HashMap::<String, Value>::new())
        .expect("macro should render")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

mod json_parse {
    use super::*;

    const TEMPLATE: &str = "{{ json_parse('json_string', ['address', 'city']) }}";

    #[test]
    fn postgres_uses_json_path_operator() {
        let rendered = render(AdapterType::Postgres, TEMPLATE);
        assert_eq!(rendered, "json_string::json #>> '{address,city}'");
    }

    #[test]
    fn redshift_returns_null_for_invalid_json() {
        let rendered = render(AdapterType::Redshift, TEMPLATE);
        assert!(
            rendered.starts_with("case when is_valid_json(json_string)"),
            "Redshift json_parse must guard against invalid JSON, got: {rendered}"
        );
        assert!(
            rendered.contains("json_extract_path_text(json_string,'address','city')"),
            "got: {rendered}"
        );
    }

    #[test]
    fn snowflake_returns_text() {
        let rendered = render(AdapterType::Snowflake, TEMPLATE);
        assert_eq!(
            rendered,
            "cast(try_parse_json(json_string)['address']['city'] as varchar)"
        );
    }

    #[test]
    fn bigquery_uses_json_extract_scalar() {
        let rendered = render(AdapterType::Bigquery, TEMPLATE);
        assert_eq!(rendered, "json_extract_scalar(json_string, '$.address.city')");
    }

    #[test]
    fn spark_uses_get_json_object() {
        let rendered = render(AdapterType::Spark, TEMPLATE);
        assert_eq!(rendered, "get_json_object(json_string, '$.address.city')");
    }

    /// Numbers in `string_path` are array indexes, not quoted keys.
    const ARRAY_TEMPLATE: &str = "{{ json_parse('json_string', ['data', 0, 'id']) }}";

    #[test]
    fn snowflake_indexes_arrays_with_numbers() {
        let rendered = render(AdapterType::Snowflake, ARRAY_TEMPLATE);
        assert_eq!(
            rendered,
            "cast(try_parse_json(json_string)['data'][0]['id'] as varchar)"
        );
    }

    #[test]
    fn bigquery_indexes_arrays_with_numbers() {
        let rendered = render(AdapterType::Bigquery, ARRAY_TEMPLATE);
        assert_eq!(rendered, "json_extract_scalar(json_string, '$.data[0].id')");
    }

    #[test]
    fn spark_indexes_arrays_with_numbers() {
        let rendered = render(AdapterType::Spark, ARRAY_TEMPLATE);
        assert_eq!(rendered, "get_json_object(json_string, '$.data[0].id')");
    }
}

mod percentile {
    use super::*;

    const TEMPLATE: &str = "{{ percentile('value_field', 'key_column', '0.5') }}";

    #[test]
    fn snowflake_uses_default_window_function() {
        let rendered = render(AdapterType::Snowflake, TEMPLATE);
        assert_eq!(
            rendered,
            "percentile_cont(0.5) within group (order by value_field) over (partition by key_column)"
        );
    }

    #[test]
    fn postgres_is_an_aggregate() {
        let rendered = render(AdapterType::Postgres, TEMPLATE);
        assert_eq!(
            rendered,
            "percentile_cont(0.5) within group (order by value_field)"
        );
    }

    #[test]
    fn redshift_casts_to_float_and_uses_a_window() {
        let rendered = render(AdapterType::Redshift, TEMPLATE);
        assert_eq!(
            rendered,
            "percentile_cont(0.5) within group (order by cast(value_field as float)) over (partition by key_column)"
        );
    }

    #[test]
    fn bigquery_uses_analytic_percentile_cont() {
        let rendered = render(AdapterType::Bigquery, TEMPLATE);
        assert_eq!(
            rendered,
            "percentile_cont(value_field, 0.5) over (partition by key_column)"
        );
    }

    #[test]
    fn spark_uses_percentile() {
        let rendered = render(AdapterType::Spark, TEMPLATE);
        assert_eq!(
            rendered,
            "percentile(value_field, 0.5) over (partition by key_column)"
        );
    }
}
