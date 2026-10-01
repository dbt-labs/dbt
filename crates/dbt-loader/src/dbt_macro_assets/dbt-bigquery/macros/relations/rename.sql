{# DIVERGENCE: in core, rename_relation is implemented via the googleapi inside the adapter.rename_relation method. #}
{% macro bigquery__rename_relation(from_relation, to_relation) -%}
  {% set target_name = adapter.quote_as_configured(to_relation.identifier, 'identifier') %}
  {% call statement('rename_relation') -%}
    alter table {{ from_relation.render() }} rename to {{ target_name }}
  {%- endcall %}
{% endmacro %}
