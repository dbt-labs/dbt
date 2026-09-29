{% macro exasol__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    {%- if limit_num -%}
        {#- Exasol has no array_slice/array_to_string to cap the row count;
            the default's limit_num path can't be expressed. -#}
        {{ exceptions.raise_compiler_error("listagg(limit_num=...) is not supported on Exasol; filter rows in a subquery/WHERE instead. (Exasol LISTAGG itself has a 2,000,000-char result limit.)") }}
    {%- endif -%}
    listagg(
        {{ measure }},
        {{ delimiter_text }}
    )
    {%- if order_by_clause %} within group ({{ order_by_clause }}){% endif -%}
{%- endmacro %}
