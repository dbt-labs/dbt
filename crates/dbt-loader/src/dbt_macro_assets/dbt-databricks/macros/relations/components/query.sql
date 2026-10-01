{% macro alter_query(target_relation, query) %}
  {{ log("Altering query") }}
  {% if query %}
    {% call statement('main') %}
      {{- get_alter_query_sql(target_relation, query) }}
    {% endcall %}
  {% endif %}
{% endmacro %}

{% macro get_alter_query_sql(target_relation, query) -%}
  {#- DIVERGENCE BEGIN: upstream uses target_relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(target_relation.type) }} {{ target_relation.render() }} AS (
  {#- DIVERGENCE END -#}
    {{ query }}
  )
{%- endmacro %}