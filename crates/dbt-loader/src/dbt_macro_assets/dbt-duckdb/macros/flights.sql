{% macro flight_name(parsed_model) -%}
  {{ return(adapter.dispatch('flight_name', 'dbt')(parsed_model)) }}
{%- endmacro %}

{% macro default__flight_name(parsed_model) -%}
  {{ return(duckdb__flight_name(parsed_model)) }}
{%- endmacro %}

{% macro duckdb__flight_name(parsed_model) -%}
  {%- set parts = ['dbt'] -%}
  {%- for key in ['package_name', 'database', 'schema'] -%}
    {%- if parsed_model.get(key) -%}
      {%- do parts.append(parsed_model.get(key)) -%}
    {%- endif -%}
  {%- endfor -%}
  {%- set identifier = parsed_model.get('alias') or parsed_model.get('name') -%}
  {%- if identifier -%}
    {%- do parts.append(identifier) -%}
  {%- endif -%}
  {{ return(parts | join('-')) }}
{%- endmacro %}
