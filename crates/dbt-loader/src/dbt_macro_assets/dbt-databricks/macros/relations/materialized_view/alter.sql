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
        {% if configuration_changes.changes["partition_by"] %}
            {{- log('Applying REPLACE to: ' ~ existing_relation) -}}
            {% do return(drop_and_create(existing_relation, relation, sql)) %}
        {% else %}
            {% do return(get_replace_sql(existing_relation, relation, sql)) %}
        {% endif %}

    -- otherwise apply individual changes as needed
    {% else %}
        {%- set alter_statement = get_alter_mv_internal(relation, configuration_changes) -%}
        {%- set return_statements = [] -%}
        {%- if alter_statement -%}
            {{ return_statements.append(alter_statement) }}
        {%- endif -%}
        {%- set tags = configuration_changes.changes["tags"] -%}
        {%- if tags and tags.set_tags and tags.set_tags != [] -%}
            {{ return_statements.append(alter_set_tags(relation, tags.set_tags)) }}
        {%- endif -%}

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
