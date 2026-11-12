//! Converts CLI `--vars` YAML values into Jinja values.
//!
//! dbt-yaml lacks YAML 1.1 timestamp resolution, so CLI `--vars` date
//! scalars become strings and `.strftime()` fails. We wrap them as Jinja
//! date objects to match dbt Core's behavior.
use chrono::NaiveDate;
use minijinja::Value as MinijinjaValue;
use minijinja::value::ValueMap;
use minijinja_contrib::modules::py_datetime::date::PyDate;

pub(crate) fn cli_var_value_to_minijinja(value: &dbt_yaml::Value) -> MinijinjaValue {
    match value {
        dbt_yaml::Value::String(s, _) => string_to_minijinja(s),
        dbt_yaml::Value::Sequence(seq, _) => {
            let items: Vec<MinijinjaValue> = seq.iter().map(cli_var_value_to_minijinja).collect();
            MinijinjaValue::from_serialize(&items)
        }
        dbt_yaml::Value::Mapping(map, _) => {
            let mut result = ValueMap::with_capacity(map.len());
            for (k, v) in map {
                result.insert(cli_var_value_to_minijinja(k), cli_var_value_to_minijinja(v));
            }
            MinijinjaValue::from_serialize(&result)
        }
        dbt_yaml::Value::Tagged(tagged, _) => cli_var_value_to_minijinja(&tagged.value),
        // Null, Bool, and Number carry no nested dates, so hand them straight
        // to minijinja's own serializer rather than reimplementing it here.
        _ => MinijinjaValue::from_serialize(value),
    }
}

fn string_to_minijinja(s: &str) -> MinijinjaValue {
    match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        Ok(date) => MinijinjaValue::from_object(PyDate::new(date)),
        Err(_) => MinijinjaValue::from(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn parse_vars(yaml: &str) -> BTreeMap<String, dbt_yaml::Value> {
        dbt_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn unquoted_date_scalar_becomes_a_jinja_date_object() {
        let vars = parse_vars("start_date: 2026-08-03\n");
        let value = cli_var_value_to_minijinja(&vars["start_date"]);
        assert!(value.downcast_object_ref::<PyDate>().is_some());
        assert_eq!(value.to_string(), "2026-08-03");

        let env = minijinja::Environment::new();
        let template = env
            .template_from_str("{{ start_date.strftime('%Y%m') }}")
            .unwrap();
        let rendered = template
            .render(minijinja::context!(start_date => value), &[])
            .unwrap();
        assert_eq!(rendered, "202608");
    }

    #[test]
    fn quoted_date_scalar_is_also_treated_as_a_date() {
        // dbt-yaml loses quoting info, making quoted/unquoted dates indistinguishable.
        // This PR diverges from Core by treating both as dates. Silent behavior change:
        // var("x") == "2026-08-03" now returns False. See dbt-core issue #15870.
        let vars = parse_vars("start_date: \"2026-08-03\"\n");
        let value = cli_var_value_to_minijinja(&vars["start_date"]);
        assert!(value.downcast_object_ref::<PyDate>().is_some());
    }

    #[test]
    fn non_date_string_is_unaffected() {
        let vars = parse_vars("name: jaffle_shop\n");
        let value = cli_var_value_to_minijinja(&vars["name"]);
        assert!(value.as_str().is_some());
        assert_eq!(value.to_string(), "jaffle_shop");
    }

    #[test]
    fn sequence_renders_as_a_list_not_a_tuple() {
        let vars = parse_vars("xs: [1, 2, 3]\n");
        let value = cli_var_value_to_minijinja(&vars["xs"]);
        assert_eq!(value.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn mapping_preserves_insertion_order() {
        let vars = parse_vars("m: {zeta: 1, alpha: 2}\n");
        let value = cli_var_value_to_minijinja(&vars["m"]);
        assert_eq!(value.to_string(), "{'zeta': 1, 'alpha': 2}");
    }

    #[test]
    fn mapping_with_non_string_keys_is_preserved() {
        let vars = parse_vars("m: {1: one, 2: two}\n");
        let value = cli_var_value_to_minijinja(&vars["m"]);
        assert_eq!(value.len().unwrap(), 2);
        assert_eq!(
            value.get_item(&MinijinjaValue::from(1)).unwrap().as_str(),
            Some("one")
        );
    }

    #[test]
    fn large_u64_keeps_full_precision() {
        let vars = parse_vars("n: 9223372036854775808\n");
        let value = cli_var_value_to_minijinja(&vars["n"]);
        assert_eq!(value.to_string(), "9223372036854775808");
    }

    #[test]
    fn nested_date_inside_a_sequence_is_still_converted() {
        let vars = parse_vars("xs: [2026-08-03, not_a_date]\n");
        let value = cli_var_value_to_minijinja(&vars["xs"]);
        let first = value.get_item(&MinijinjaValue::from(0)).unwrap();
        assert!(first.downcast_object_ref::<PyDate>().is_some());
    }

    #[test]
    fn nested_date_inside_a_mapping_is_still_converted() {
        let vars = parse_vars("m: {start_date: 2026-08-03}\n");
        let value = cli_var_value_to_minijinja(&vars["m"]);
        let start_date = value.get_item(&MinijinjaValue::from("start_date")).unwrap();
        assert!(start_date.downcast_object_ref::<PyDate>().is_some());
    }
}
