-- funcsign: (relation, snowflake_node_config, relation, string) -> string
{% macro snowflake__get_alter_interactive_table_as_sql(
    existing_relation,
    configuration_changes,
    target_relation,
    sql
) -%}
    {{- log('Applying ALTER to: ' ~ existing_relation) -}}

    {#- DIVERGENCE: core decides requires_full_refresh in the materialization and only reaches this
        macro on the ALTER path; Fusion branches here instead, mirroring
        snowflake__get_alter_dynamic_table_as_sql. -#}

    {% if configuration_changes.requires_full_refresh %}
        {{- get_replace_sql(existing_relation, target_relation, sql) -}}

    {% else %}

        {{- snowflake__get_target_lag_warehouse_alter_sql('interactive', existing_relation, configuration_changes) -}}

        {#- No `cluster_by` statement here: Snowflake rejects `alter ... cluster by` on an
            interactive table, so it comes through as requires_full_refresh above. -#}

    {%- endif -%}

{%- endmacro %}
