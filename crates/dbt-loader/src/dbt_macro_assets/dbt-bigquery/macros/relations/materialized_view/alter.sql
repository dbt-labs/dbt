{% macro bigquery__get_alter_materialized_view_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}

    {% if configuration_changes.requires_full_refresh %}
        {{ get_replace_sql(existing_relation, relation, sql) }}
    {% else %}

        alter materialized view {{ relation }}
            set {{ bigquery_options(configuration_changes.options.context.as_ddl_dict()) }}

    {%- endif %}

{% endmacro %}

{% macro bigquery__get_materialized_view_configuration_changes(existing_relation, new_config) %}
    {% set _existing_materialized_view = adapter.describe_relation(existing_relation) %}
    {% set _configuration_changes = existing_relation.materialized_view_config_changeset(_existing_materialized_view, new_config.model) %}
    {% do return(_configuration_changes) %}
{% endmacro %}
