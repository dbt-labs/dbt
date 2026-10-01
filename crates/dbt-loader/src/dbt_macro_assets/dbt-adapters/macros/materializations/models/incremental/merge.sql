-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) -%}
   -- back compat for old kwarg name
  {% set incremental_predicates = kwargs.get('predicates', incremental_predicates) %}
  {{ adapter.dispatch('get_merge_sql', 'dbt')(target, source, unique_key, dest_columns, incremental_predicates) }}
{%- endmacro %}

-- funcsign: (string, string) -> string
{% macro get_merge_unique_key_match(source_unique_key, target_unique_key) %}
  {{ return(adapter.dispatch('get_merge_unique_key_match', 'dbt')(source_unique_key, target_unique_key)) }}
{% endmacro %}

-- funcsign: (string, string) -> string
{% macro default__get_merge_unique_key_match(source_unique_key, target_unique_key) -%}
    {{ return(equals(source_unique_key, target_unique_key) | trim) }}
{%- endmacro %}

-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro default__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) -%}
    {%- set predicates = [] if incremental_predicates is none else [] + incremental_predicates -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    {%- set merge_update_columns = config.get('merge_update_columns') -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
    {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
            {% for key in unique_key %}
                {% set this_key_match %}
                    DBT_INTERNAL_SOURCE.{{ key }} = DBT_INTERNAL_DEST.{{ key }}
                {% endset %}
                {% do predicates.append(this_key_match) %}
            {% endfor %}
        {% else %}
            {% set source_unique_key = ("DBT_INTERNAL_SOURCE." ~ unique_key) | trim %}
            {% set target_unique_key = ("DBT_INTERNAL_DEST." ~ unique_key) | trim %}
            {% set unique_key_match = get_merge_unique_key_match(source_unique_key, target_unique_key) %}
            {% do predicates.append(unique_key_match) %}
        {% endif %}
    {% else %}
        {% do predicates.append('FALSE') %}
    {% endif %}

    {{ sql_header if sql_header is not none }}

    merge into {{ target }} as DBT_INTERNAL_DEST
        using {{ source }} as DBT_INTERNAL_SOURCE
        on {{"(" ~ predicates | join(") and (") ~ ")"}}

    {% if unique_key %}
    when matched then update set
        {% for column_name in update_columns -%}
            {{ column_name }} = DBT_INTERNAL_SOURCE.{{ column_name }}
            {%- if not loop.last %}, {%- endif %}
        {%- endfor %}
    {% endif %}

    when not matched then insert
        ({{ dest_cols_csv }})
    values
        ({{ dest_cols_csv }})

{% endmacro %}


-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}
  {{ adapter.dispatch('get_delete_insert_merge_sql', 'dbt')(target, source, unique_key, dest_columns, incremental_predicates) }}
{%- endmacro %}

-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro default__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is string %}
        {% set unique_key = [unique_key] %}
        {% endif %}

        {%- set unique_key_str = unique_key|join(', ') -%}

        delete from {{ target }} as DBT_INTERNAL_DEST
        where ({{ unique_key_str }}) in (
            select distinct {{ unique_key_str }}
            from {{ source }} as DBT_INTERNAL_SOURCE
        )
        {%- if incremental_predicates %}
            {% for predicate in incremental_predicates %}
                and {{ predicate }}
            {% endfor %}
        {%- endif -%};

    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )

{%- endmacro %}


-- funcsign: (string, string, list[base_column], optional[list[string]], optional[bool]) -> string
{% macro get_insert_overwrite_merge_sql(target, source, dest_columns, predicates, include_sql_header=false) -%}
  {{ adapter.dispatch('get_insert_overwrite_merge_sql', 'dbt')(target, source, dest_columns, predicates, include_sql_header) }}
{%- endmacro %}

-- funcsign: (string, string, list[base_column], optional[list[string]], optional[bool]) -> string
{% macro default__get_insert_overwrite_merge_sql(target, source, dest_columns, predicates, include_sql_header) -%}
    {#-- The only time include_sql_header is True: --#}
    {#-- BigQuery + insert_overwrite strategy + "static" partitions config --#}
    {#-- We should consider including the sql header at the materialization level instead --#}

    {%- set predicates = [] if predicates is none else [] + predicates -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {{ sql_header if sql_header is not none and include_sql_header }}

    merge into {{ target }} as DBT_INTERNAL_DEST
        using {{ source }} as DBT_INTERNAL_SOURCE
        on FALSE

    when not matched by source
        {% if predicates %} and {{ predicates | join(' and ') }} {% endif %}
        then delete

    when not matched then insert
        ({{ dest_cols_csv }})
    values
        ({{ dest_cols_csv }})

{% endmacro %}
