{% macro postgres__get_create_materialized_view_as_sql(relation, sql) %}
    create materialized view if not exists {{ relation }} as {{ sql }};

    {% for _index_dict in config.get('indexes', []) -%}
        {{- get_create_index_sql(relation, _index_dict) -}}{{ ';' if not loop.last else "" }}
    {%- endfor -%}

{% endmacro %}
