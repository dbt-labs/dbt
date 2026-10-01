{% macro fabric__get_limit_sql(sql, limit) %}
    {%- if limit == -1 or limit is none -%}
        {{ sql }}
    {#- Special processing if the last non-blank line starts with order by -#}
    {%- elif sql.strip().splitlines()[-1].strip().lower().startswith('order by') -%}
        {{ sql }}
        offset 0 rows  fetch first {{ limit }} rows only
    {%- else -%}
        {{ sql }}
        order by (select null) offset 0 rows fetch first {{ limit }} rows only
    {%- endif -%}
{% endmacro %}
