{% macro exasol__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) -%}
    {#- Same as default__get_merge_sql, except the ON predicate uses a plain `=`
        instead of the dispatched equals() macro. exasol__equals renders a
        DECODE(...) = 1 for NULL-safe equality, which Exasol rejects in a MERGE
        condition ("such a merge condition is not supported!"). A plain `=` is
        what Exasol's MERGE requires; '' is NULL on Exasol so empty-string keys
        behave the same either way. -#}
    {%- set predicates = [] if incremental_predicates is none else [] + incremental_predicates -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    {%- set merge_update_columns = config.get('merge_update_columns') -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
    {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {#- Exasol forbids updating a column that appears in the MERGE ON condition
        ("updating column of ON-condition is not allowed in MERGE!"), so drop the
        unique_key column(s) from the update set. -#}
    {%- set key_list = [unique_key] if unique_key is string else (unique_key or []) -%}
    {%- set key_upper = key_list | map('upper') | list -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
            {% for key in unique_key %}
                {% set this_key_match %}
                    DBT_INTERNAL_SOURCE.{{ key }} = DBT_INTERNAL_DEST.{{ key }}
                {% endset %}
                {% do predicates.append(this_key_match) %}
            {% endfor %}
        {% else %}
            {% set this_key_match %}
                DBT_INTERNAL_SOURCE.{{ unique_key }} = DBT_INTERNAL_DEST.{{ unique_key }}
            {% endset %}
            {% do predicates.append(this_key_match) %}
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
        {% for column_name in update_columns if (column_name | replace('"', '') | upper) not in key_upper -%}
            {{ column_name }} = DBT_INTERNAL_SOURCE.{{ column_name }}
            {%- if not loop.last %}, {%- endif %}
        {%- endfor %}
    {% endif %}

    when not matched then insert
        ({{ dest_cols_csv }})
    values
        ({{ dest_cols_csv }})

{% endmacro %}
