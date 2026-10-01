{% macro redshift__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}
    {%- set predicates = _update_predicates(target, incremental_predicates) -%}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is string %}
            {% set unique_key = [unique_key] %}
        {% endif %}

        {%- set unique_key_str = unique_key | join(', ') -%}

    delete from {{ target }}
    where ({{ unique_key_str }}) in (
        select distinct {{ unique_key_str }}
        from {{ source }} as DBT_INTERNAL_SOURCE
    )
    {% for predicate in predicates %}
    and {{ predicate }}
    {% endfor %}
    ;
    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
        (
            select {{ dest_cols_csv }}
            from {{ source }}
        )

{%- endmacro %}
