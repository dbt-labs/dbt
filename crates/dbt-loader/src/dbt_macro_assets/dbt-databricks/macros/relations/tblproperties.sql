{% macro tblproperties_clause() -%}
  {{ return(adapter.dispatch('tblproperties_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro databricks__tblproperties_clause(tblproperties=None) -%}
  {%- if adapter.is_uniform(config) -%}
    {%- set tblproperties = adapter.update_tblproperties_for_uniform_iceberg(config, tblproperties) -%}
  {%- else -%}
    {%- set tblproperties = tblproperties or config.get("tblproperties", {}) -%}
  {%- endif -%}
  {%- if tblproperties != {} %}
    tblproperties (
      {%- for prop in tblproperties -%}
      '{{ prop }}' = '{{ tblproperties[prop] }}' {% if not loop.last %}, {% endif %}
      {%- endfor %}
    )
  {%- endif %}
{%- endmacro -%}

{% macro apply_tblproperties(relation, tblproperties) -%}
  {% set tblproperty_statment = databricks__tblproperties_clause(tblproperties) %}
  {% if tblproperty_statment %}
    {%- call statement('main') -%}
      {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
      ALTER {{ render_type(relation.type) }} {{ relation.render() }} SET {{ tblproperty_statment}}
      {#- DIVERGENCE END -#}
    {%- endcall -%}
  {% endif %}
{%- endmacro -%}
