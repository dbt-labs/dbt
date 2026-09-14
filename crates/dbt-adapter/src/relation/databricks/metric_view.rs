use regex::Regex;
use std::sync::LazyLock;

static YAML_SOURCE_BACKTICK_VALUE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^(?P<before> *source *: +)(?P<value>`[^`\n]+`(?:\.`[^`\n]+`)*)(?P<after> *(?:#[^\n]*)?)$",
    )
    .expect("valid metric-view source regex")
});

/// Reference: https://github.com/databricks/dbt-databricks/blob/2c3aa9fdddbab30a3c4a660c5e98722e989a592b/dbt/adapters/databricks/handle.py#L374
pub(crate) fn quote_metric_view_sources(yaml_body: &str) -> String {
    if !yaml_body.contains('`') {
        return yaml_body.to_string();
    }

    YAML_SOURCE_BACKTICK_VALUE_REGEX
        .replace_all(yaml_body, "${before}\"${value}\"${after}")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::quote_metric_view_sources;

    #[test]
    fn quotes_only_bare_backtick_source_identifiers() {
        let yaml = concat!(
            "source: `catalog`.`schema`.`orders`\n",
            "joins:\n",
            "  - name: customers\n",
            "    source :   `catalog`.`schema`.`customers` # customer source\n",
            "    on: `orders`.`customer_id` = `customers`.`id`\n",
            "data_source: `catalog`.`schema`.`ignored`\n",
        );
        let expected = concat!(
            "source: \"`catalog`.`schema`.`orders`\"\n",
            "joins:\n",
            "  - name: customers\n",
            "    source :   \"`catalog`.`schema`.`customers`\" # customer source\n",
            "    on: `orders`.`customer_id` = `customers`.`id`\n",
            "data_source: `catalog`.`schema`.`ignored`\n",
        );

        assert_eq!(quote_metric_view_sources(yaml), expected);
        assert_eq!(
            quote_metric_view_sources("source: \"`catalog`.`schema`.`orders`\"\n"),
            "source: \"`catalog`.`schema`.`orders`\"\n"
        );
    }
}
