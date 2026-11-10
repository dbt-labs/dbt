{% macro exasol__get_empty_subquery_sql(select_sql, select_sql_header=none) %}
    {#- default__get_empty_subquery_sql aliases the subquery as `__dbt_sbq`.
        Exasol rejects a leading underscore in an unquoted identifier
        ("unexpected invalid token, expecting IDENTIFIER_PART_"), so use a plain
        alias. Drives column-schema detection (get_columns_in_query,
        check_time_data_types, contracts), so it must parse on Exasol. -#}
    {%- if select_sql_header is not none -%}
    {{ select_sql_header }}
    {%- endif -%}
    select * from (
        {{ select_sql }}
    ) dbt_sbq
    where false
    limit 0
{% endmacro %}

{% macro exasol__alter_column_type(relation, column_name, new_column_type) -%}
    {#- Exasol supports in-place MODIFY COLUMN; the default's add/copy/drop/rename
        dance is unnecessary and its multi-statement block would not execute. -#}
    {% call statement('alter_column_type') %}
        alter table {{ relation.render() }} modify column {{ adapter.quote(column_name) }} {{ new_column_type }}
    {% endcall %}
{% endmacro %}

{% macro exasol__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}
    {#- Exasol allows only one ADD/DROP COLUMN action per ALTER statement and no
        multi-statement execute, so issue one statement per column. -#}
    {% if add_columns is none %}
        {% set add_columns = [] %}
    {% endif %}
    {% if remove_columns is none %}
        {% set remove_columns = [] %}
    {% endif %}

    {% for column in add_columns %}
        {% set sql -%}
            alter {{ relation.type }} {{ relation.render() }} add column {{ column.name }} {{ column.data_type }}
        {%- endset %}
        {% do run_query(sql) %}
    {% endfor %}

    {% for column in remove_columns %}
        {% set sql -%}
            alter {{ relation.type }} {{ relation.render() }} drop column {{ column.name }}
        {%- endset %}
        {% do run_query(sql) %}
    {% endfor %}

{% endmacro %}
