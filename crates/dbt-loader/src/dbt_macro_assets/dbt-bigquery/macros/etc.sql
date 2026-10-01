{% macro date_sharded_table(base_name) %}
    {{ return(base_name ~ "[DBT__PARTITION_DATE]") }}
{% endmacro %}

{% macro grant_access_to(entity, entity_type, role, grant_target_dict) -%}
  {% do adapter.grant_access_to(entity, entity_type, role, grant_target_dict) %}
{% endmacro %}

{%- macro get_partitions_metadata(table) -%}
  {%- if execute -%}
    {%- set res = adapter.get_partitions_metadata(table) -%}
    {{- return(res) -}}
  {%- endif -%}
  {{- return(None) -}}
{%- endmacro -%}
