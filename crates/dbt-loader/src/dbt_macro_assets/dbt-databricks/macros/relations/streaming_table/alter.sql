{% macro get_alter_streaming_table_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}
    {{- log('Applying ALTER to: ' ~ relation) -}}
    {%- do return(adapter.dispatch('get_alter_streaming_table_as_sql', 'dbt')(
        relation,
        configuration_changes,
        sql,
        existing_relation,
        backup_relation,
        intermediate_relation
    )) -%}
{% endmacro %}

{% macro databricks__get_alter_streaming_table_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}
    -- apply a full refresh immediately if needed
    {% if configuration_changes.requires_full_refresh %}
        {% do return(get_replace_sql(existing_relation, relation,  sql)) %}

    -- otherwise apply individual changes as needed
    {% else %}
        {%- set alter_statement = get_alter_st_internal(relation, configuration_changes) -%}
        {%- set create_statement = get_create_st_internal(relation, configuration_changes, sql) -%}
        {%- set return_statements = [] -%}
        {%- if create_statement -%}
            {{ return_statements.append(create_statement) }}
        {%- endif -%}
        {%- if alter_statement -%}
            {{ return_statements.append(alter_statement) }}
        {%- endif -%}
        {%- set tags = configuration_changes.changes["tags"] -%}
        {%- if tags and tags.set_tags and tags.set_tags != [] -%}
            {{ return_statements.append(alter_set_tags(relation, tags.set_tags)) }}
        {%- endif -%}

        {#- Row filter handling - append SQL to list, don't execute -#}
        {#- is_change guard prevents false alters from `diff or value` fallback in streaming_table.py:56 -#}
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

{% macro get_create_st_internal(relation, configuration_changes, sql) %}
  {%- set partition_by = configuration_changes.changes["partition_by"].partition_by -%}
  {%- set tblproperties = configuration_changes.changes["tblproperties"].tblproperties -%}
  {%- set comment = configuration_changes.changes["comment"].comment -%}
  CREATE OR REFRESH STREAMING TABLE {{ relation.render() }}
    {% if partition_by -%}
        {{ get_create_sql_partition_by(partition_by) }}
    {%- endif %}
    {% if comment -%}
        {{ get_create_sql_comment(comment) }}
    {%- endif %}
    {% if tblproperties -%}
        {{ get_create_sql_tblproperties(tblproperties) }}
    {%- endif %}
    AS {{ sql }}
{% endmacro %}

{% macro get_alter_st_internal(relation, configuration_changes) %}
  {%- set refresh = configuration_changes.changes["refresh"] -%}
  {%- set is_scheduled = refresh and (refresh.cron or refresh.every or refresh.on_update) -%}
  {%- if is_scheduled -%}
    ALTER STREAMING TABLE {{ relation.render() }}
        ADD {{ get_create_sql_refresh_schedule(refresh) -}}
  {%- endif -%}
{% endmacro %}
