{% macro get_materialized_view_configuration_changes(existing_relation, new_config) %}
    /* {#
    It's recommended that configuration changes be formatted as follows:
    {"<change_category>": [{"action": "<name>", "context": ...}]}

    For example:
    {
        "indexes": [
            {"action": "drop", "context": "index_abc"},
            {"action": "create", "context": {"columns": ["column_1", "column_2"], "type": "hash", "unique": True}},
        ],
    }

    Either way, `get_materialized_view_configuration_changes` needs to align with `get_alter_materialized_view_as_sql`.
    #} */
    {{- log('Determining configuration changes on: ' ~ existing_relation) -}}
    {%- do return(adapter.dispatch('get_materialized_view_configuration_changes', 'dbt')(existing_relation, new_config)) -%}
{% endmacro %}


{% macro default__get_materialized_view_configuration_changes(existing_relation, new_config) %}
    {{ exceptions.raise_compiler_error("Materialized views have not been implemented for this adapter.") }}
{% endmacro %}
