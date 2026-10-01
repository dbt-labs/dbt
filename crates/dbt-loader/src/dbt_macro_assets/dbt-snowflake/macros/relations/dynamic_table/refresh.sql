{% macro snowflake__refresh_dynamic_table(relation) -%}
    {{- log('Applying REFRESH to: ' ~ relation) -}}

    alter dynamic table {{ relation }} refresh
{%- endmacro %}
