//! GRANT, REVOKE and SHOW GRANTS statements for function relations.
//!
//! `grant <privilege> on <name>` means `on table <name>` on Snowflake,
//! Databricks, Postgres and Redshift, so a grant on a function used to fail with
//! "table does not exist". These tests render the real adapter macros for a
//! function relation and check that the statement names the object as a
//! function, with argument types where the warehouse needs them. Table
//! relations must keep the statement they had before.

use std::collections::BTreeMap;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const GRANT: &str = "{{ get_grant_sql(relation, privilege, grantees) }}";
const REVOKE: &str = "{{ get_revoke_sql(relation, privilege, grantees) }}";
const SHOW: &str = "{{ get_show_grant_sql(relation) }}";
const RENDERED_NAME: &str = "{{ relation.render() }}";

fn harness(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let quote_char = match adapter_type {
        AdapterType::Databricks | AdapterType::Spark | AdapterType::Bigquery => '`',
        _ => '"',
    };
    harness.mock().on("quote", move |args| {
        let name = args[0].as_str().expect("identifier to quote");
        Ok(Value::from(format!("{quote_char}{name}{quote_char}")))
    });
    harness
}

/// A `model` whose `arguments` match a function `double_it(x integer, y varchar)`.
fn function_model(arg_types: &[&str]) -> Value {
    let arguments: Vec<BTreeMap<&str, &str>> = arg_types
        .iter()
        .enumerate()
        .map(|(i, data_type)| {
            BTreeMap::from([("name", ["x", "y", "z"][i]), ("data_type", *data_type)])
        })
        .collect();
    Value::from_serialize(BTreeMap::from([
        ("name", Value::from("double_it")),
        ("arguments", Value::from_serialize(arguments)),
    ]))
}

struct Rendered {
    name: String,
    sql: String,
}

fn render(
    harness: &MacroTestHarness,
    template: &str,
    relation_type: RelationType,
    model: Value,
    privilege: &str,
    grantees: &[&str],
) -> Result<Rendered, String> {
    let relation = harness.relation("TEST_DB", "TEST_SCHEMA", "double_it", Some(relation_type));
    let ctx = BTreeMap::from([
        (
            "relation".to_string(),
            RelationObject::new(relation).into_value(),
        ),
        ("model".to_string(), model),
        ("privilege".to_string(), Value::from(privilege)),
        ("grantees".to_string(), Value::from_serialize(grantees)),
    ]);
    let name = harness
        .render(RENDERED_NAME, ctx.clone())
        .expect("relation should render");
    let sql = harness
        .render(template, ctx)
        .map_err(|e| format!("{e:?}"))?;
    Ok(Rendered {
        name: name.trim().to_string(),
        sql: sql.split_whitespace().collect::<Vec<_>>().join(" "),
    })
}

fn render_ok(
    harness: &MacroTestHarness,
    template: &str,
    relation_type: RelationType,
    model: Value,
    privilege: &str,
    grantees: &[&str],
) -> Rendered {
    render(harness, template, relation_type, model, privilege, grantees)
        .expect("render should succeed")
}

mod snowflake {
    use super::*;

    #[test]
    fn grant_names_function_with_argument_types() {
        let h = harness(AdapterType::Snowflake);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["integer", "varchar"]),
            "usage",
            &["public"],
        );
        assert_eq!(
            r.sql,
            format!(
                "grant usage on function {}(integer, varchar) to public",
                r.name
            )
        );
    }

    #[test]
    fn grant_names_function_without_arguments_with_empty_parens() {
        let h = harness(AdapterType::Snowflake);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&[]),
            "usage",
            &["public"],
        );
        assert_eq!(
            r.sql,
            format!("grant usage on function {}() to public", r.name)
        );
    }

    /// Overloads are materialized one at a time with `model.arguments` set to
    /// that overload's signature, so each one gets its own statement.
    #[test]
    fn each_overload_signature_gets_its_own_grant() {
        let h = harness(AdapterType::Snowflake);
        let a = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["integer"]),
            "usage",
            &["public"],
        );
        let b = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["float"]),
            "usage",
            &["public"],
        );
        assert_eq!(
            a.sql,
            format!("grant usage on function {}(integer) to public", a.name)
        );
        assert_eq!(
            b.sql,
            format!("grant usage on function {}(float) to public", b.name)
        );
    }

    #[test]
    fn revoke_and_show_name_function_with_argument_types() {
        let h = harness(AdapterType::Snowflake);
        let revoke = render_ok(
            &h,
            REVOKE,
            RelationType::Function,
            function_model(&["integer"]),
            "usage",
            &["public"],
        );
        assert_eq!(
            revoke.sql,
            format!(
                "revoke usage on function {}(integer) from \"public\"",
                revoke.name
            )
        );
        let show = render_ok(
            &h,
            SHOW,
            RelationType::Function,
            function_model(&["integer"]),
            "usage",
            &["public"],
        );
        assert_eq!(
            show.sql,
            format!("show grants on function {}(integer)", show.name)
        );
    }

    #[test]
    fn table_grant_is_unchanged() {
        let h = harness(AdapterType::Snowflake);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Table,
            function_model(&[]),
            "select",
            &["reporter"],
        );
        assert_eq!(r.sql, format!("grant select on {} to reporter", r.name));
    }
}

mod databricks {
    use super::*;

    #[test]
    fn grant_and_revoke_name_function_without_argument_types() {
        let h = harness(AdapterType::Databricks);
        let grant = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["int"]),
            "execute",
            &["account users"],
        );
        assert_eq!(
            grant.sql,
            format!(
                "grant execute on function {} to `account users`",
                grant.name
            )
        );
        let revoke = render_ok(
            &h,
            REVOKE,
            RelationType::Function,
            function_model(&["int"]),
            "execute",
            &["account users"],
        );
        assert_eq!(
            revoke.sql,
            format!(
                "revoke execute on function {} from `account users`",
                revoke.name
            )
        );
    }

    #[test]
    fn show_names_function() {
        let h = harness(AdapterType::Databricks);
        let r = render_ok(
            &h,
            SHOW,
            RelationType::Function,
            function_model(&["int"]),
            "execute",
            &[],
        );
        assert_eq!(r.sql, format!("show grants on function {}", r.name));
    }

    #[test]
    fn table_grant_is_unchanged() {
        let h = harness(AdapterType::Databricks);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Table,
            function_model(&[]),
            "select",
            &["account users"],
        );
        assert_eq!(
            r.sql,
            format!("grant select on {} to `account users`", r.name)
        );
    }
}

mod postgres {
    use super::*;

    #[test]
    fn grant_and_revoke_name_function_with_argument_types() {
        let h = harness(AdapterType::Postgres);
        let grant = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["integer", "text"]),
            "execute",
            &["reporter"],
        );
        assert_eq!(
            grant.sql,
            format!(
                "grant execute on function {}(integer, text) to reporter",
                grant.name
            )
        );
        let revoke = render_ok(
            &h,
            REVOKE,
            RelationType::Function,
            function_model(&["integer", "text"]),
            "execute",
            &["reporter"],
        );
        assert_eq!(
            revoke.sql,
            format!(
                "revoke execute on function {}(integer, text) from \"reporter\"",
                revoke.name
            )
        );
    }

    #[test]
    fn table_grant_is_unchanged() {
        let h = harness(AdapterType::Postgres);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Table,
            function_model(&[]),
            "select",
            &["reporter"],
        );
        assert_eq!(r.sql, format!("grant select on {} to reporter", r.name));
    }
}

mod redshift {
    use super::*;

    fn set_grants_extended(h: &MacroTestHarness, enabled: bool) {
        h.mock().set_attr(
            "behavior",
            Value::from_serialize(BTreeMap::from([(
                "redshift_grants_extended",
                BTreeMap::from([("no_warn", enabled)]),
            )])),
        );
    }

    #[test]
    fn grant_names_function_with_argument_types() {
        for extended in [false, true] {
            let h = harness(AdapterType::Redshift);
            set_grants_extended(&h, extended);
            let r = render_ok(
                &h,
                GRANT,
                RelationType::Function,
                function_model(&["integer"]),
                "execute",
                &["reporter"],
            );
            assert_eq!(
                r.sql,
                format!("grant execute on function {}(integer) to reporter", r.name),
                "redshift_grants_extended = {extended}"
            );
        }
    }

    #[test]
    fn extended_grant_keeps_group_and_role_prefixes() {
        let h = harness(AdapterType::Redshift);
        set_grants_extended(&h, true);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Function,
            function_model(&["integer"]),
            "execute",
            &["group:analysts", "role:bi"],
        );
        assert_eq!(
            r.sql,
            format!(
                "grant execute on function {}(integer) to GROUP analysts, ROLE bi",
                r.name
            )
        );
    }

    #[test]
    fn revoke_names_function_with_argument_types() {
        for extended in [false, true] {
            let h = harness(AdapterType::Redshift);
            set_grants_extended(&h, extended);
            let r = render_ok(
                &h,
                REVOKE,
                RelationType::Function,
                function_model(&["integer"]),
                "execute",
                &["reporter"],
            );
            assert!(
                r.sql.starts_with(&format!(
                    "revoke execute on function {}(integer) from ",
                    r.name
                )),
                "redshift_grants_extended = {extended}: {}",
                r.sql
            );
        }
    }

    #[test]
    fn table_grant_is_unchanged() {
        let h = harness(AdapterType::Redshift);
        set_grants_extended(&h, false);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Table,
            function_model(&[]),
            "select",
            &["reporter"],
        );
        assert_eq!(r.sql, format!("grant select on {} to reporter", r.name));
    }
}

mod bigquery {
    use super::*;

    /// BigQuery's GRANT and REVOKE take only schemas, tables, views and
    /// external tables. Say so instead of sending `on function ...`.
    #[test]
    fn grant_and_revoke_on_function_raise_a_clear_error() {
        let h = harness(AdapterType::Bigquery);
        for template in [GRANT, REVOKE] {
            let err = render(
                &h,
                template,
                RelationType::Function,
                function_model(&["int64"]),
                "roles/bigquery.dataViewer",
                &["user:a@example.com"],
            )
            .err()
            .unwrap_or_else(|| panic!("{template} should fail for a function"));
            assert!(
                err.contains("BigQuery GRANT and REVOKE statements do not support functions"),
                "{template}: {err}"
            );
        }
    }

    #[test]
    fn table_grant_is_unchanged() {
        let h = harness(AdapterType::Bigquery);
        let r = render_ok(
            &h,
            GRANT,
            RelationType::Table,
            function_model(&[]),
            "roles/bigquery.dataViewer",
            &["user:a@example.com"],
        );
        assert!(
            r.sql
                .starts_with("grant `roles/bigquery.dataViewer` on table "),
            "{}",
            r.sql
        );
    }
}
