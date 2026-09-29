{% macro sqlserver__length(expression) %}

    len( {{ expression }} )

{%- endmacro -%}
