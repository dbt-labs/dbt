{% macro generate_schema_name(custom_schema_name, node) -%}
    {{ return(target.schema.lower() ) }}
{%- endmacro %}
