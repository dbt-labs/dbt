{% macro get_alter_materialized_view_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}
    {{- log('Applying ALTER to: ' ~ relation) -}}
    {%- do return(adapter.dispatch('get_alter_materialized_view_as_sql', 'dbt')(
        relation,
        configuration_changes,
        sql,
        existing_relation,
        backup_relation,
        intermediate_relation
    )) -%}
{% endmacro %}

{% macro databricks__get_alter_materialized_view_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}
    -- apply a full refresh immediately if needed
    {% if configuration_changes.requires_full_refresh %}
        {#- CREATE OR REPLACE cannot change partition_by, so use DROP + CREATE when partition_by changes -#}
        {% if configuration_changes.changes["partitioned_by"] %}
            {{- log('Applying REPLACE to: ' ~ existing_relation) -}}
            {% set replace_sql = drop_and_create(existing_relation, relation, sql) %}
        {% else %}
            {% set replace_sql = get_replace_sql(existing_relation, relation, sql) %}
        {% endif %}
        {%- set return_statements = [replace_sql] if replace_sql is string else replace_sql | list -%}
        {%- set tags = config.get('databricks_tags') -%}
        {%- set column_tags = adapter.get_column_tags_from_model(config.model) -%}
        {%- do return_statements.extend(get_set_tag_statements(relation, tags, column_tags)) -%}
        {% do return(return_statements) %}

    -- otherwise apply individual changes as needed
    {% else %}
        {%- set alter_statement = get_alter_mv_internal(relation, configuration_changes) -%}
        {%- set return_statements = [] -%}
        {%- if alter_statement -%}
            {{ return_statements.append(alter_statement) }}
        {%- endif -%}
        {%- set tags = configuration_changes.changes.get("tags") -%}
        {%- set column_tags = configuration_changes.changes.get("column_tags") -%}
        {%- set set_tags = tags.set_tags if tags else none -%}
        {%- do return_statements.extend(get_set_tag_statements(relation, set_tags, column_tags)) -%}

        {#- Row filter handling - append SQL to list, don't execute -#}
        {#- is_change guard prevents false alters when row_filter is unchanged -#}
        {%- set row_filter = configuration_changes.changes.get("row_filter") -%}
        {%- if row_filter and row_filter.is_change -%}
          {%- if row_filter.should_unset -%}
            {{ return_statements.append(alter_drop_row_filter(relation)) }}
          {%- elif row_filter.function -%}
            {{ return_statements.append(alter_set_row_filter(relation, row_filter)) }}
          {%- endif -%}
        {%- endif -%}

        {#- An in-place alter replaces this run's REFRESH, so re-add it unless the server auto-refreshes -#}
        {%- set refresh = adapter.get_config_from_model(config.model)["refresh"] -%}
        {%- if not refresh.auto_refreshed -%}
          {{ return_statements.append(refresh_materialized_view(relation)) }}
        {%- endif -%}

        {% do return(return_statements) %}
    {%- endif -%}
{% endmacro %}

{% macro get_alter_mv_internal(relation, configuration_changes) %}
    {%- set refresh = configuration_changes.changes["refresh"] -%}
    {%- if refresh -%}
        -- Currently only schedule can be altered
        ALTER MATERIALIZED VIEW {{ relation.render() }}
            {{ get_alter_sql_refresh_schedule(refresh) -}}
    {%- endif -%}
{% endmacro %}
