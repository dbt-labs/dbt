{% macro sqlserver__snapshot_hash_arguments(args) %}
    CONVERT(VARCHAR(32), HashBytes('MD5', {% for arg in args %}
        coalesce(cast({{ arg }} as varchar(8000)), '') {% if not loop.last %} + '|' + {% endif %}
    {% endfor %}), 2)
{% endmacro %}


{#
    dbt-adapters does not dispatch "snapshot_check_all_get_existing_columns", so
    this shadows the global project's macro; adapter package macros take
    precedence. Keep in sync with dbt-adapters when upgrading. When upstream
    dispatches it, delete the public wrapper and keep the sqlserver__ handler.

    Change from upstream: for a check_cols list, upstream reads the columns'
    casing from "select <check_cols> from (<sql>) subq", and T-SQL can't nest a
    WITH in a subquery. This reads the query's columns unwrapped, as the 'all'
    branch does, and picks check_cols from them case-insensitively.
#}

{% macro snapshot_check_all_get_existing_columns(node, target_exists, check_cols_config) -%}
    {{ return(sqlserver__snapshot_check_all_get_existing_columns(node, target_exists, check_cols_config)) }}
{%- endmacro %}


{% macro sqlserver__snapshot_check_all_get_existing_columns(node, target_exists, check_cols_config) -%}
    {%- if not target_exists -%}
        {#-- no table yet -> return whatever the query does --#}
        {{ return((false, query_columns)) }}
    {%- endif -%}

    {#-- handle any schema changes --#}
    {%- set target_relation = adapter.get_relation(database=node.database, schema=node.schema, identifier=node.alias) -%}

    {% if check_cols_config == 'all' %}
        {%- set query_columns = get_columns_in_query(node['compiled_code']) -%}

    {% elif check_cols_config is iterable and (check_cols_config | length) > 0 %}
        {%- set all_columns = get_columns_in_query(node['compiled_code']) -%}
        {%- set all_columns_lower = all_columns | map('lower') | list -%}
        {%- set query_columns = [] -%}
        {%- for check_col in check_cols_config -%}
            {%- if (check_col | lower) not in all_columns_lower -%}
                {% do exceptions.raise_compiler_error("check_cols column '" ~ check_col ~ "' is not in the snapshot query") %}
            {%- endif -%}
            {%- do query_columns.append(all_columns[all_columns_lower.index(check_col | lower)]) -%}
        {%- endfor -%}

    {% else %}
        {% do exceptions.raise_compiler_error("Invalid value for 'check_cols': " ~ check_cols_config) %}
    {% endif %}

    {%- set existing_cols = adapter.get_columns_in_relation(target_relation) | map(attribute = 'name') | list -%}
    {%- set ns = namespace() -%} {#-- handle for-loop scoping with a namespace --#}
    {%- set ns.column_added = false -%}

    {%- set intersection = [] -%}
    {%- for col in query_columns -%}
        {%- if col in existing_cols -%}
            {%- do intersection.append(adapter.quote(col)) -%}
        {%- else -%}
            {% set ns.column_added = true %}
        {%- endif -%}
    {%- endfor -%}
    {{ return((ns.column_added, intersection)) }}
{%- endmacro %}
