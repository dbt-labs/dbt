{#- Replicates the DatabricksRelationType.render_for_alter() method from dbt-databricks.
    Uppercases and replaces underscores with spaces to produce SQL keywords
    (e.g. "materialized_view" -> "MATERIALIZED VIEW").

    DIVERGENCE: Upstream v1 dbt-databricks does not define this macro; instead each
    call site uses `relation.type.render_for_alter()` directly because v1 represents
    `relation.type` as a `DatabricksRelationType` Python enum with that method.
    In Fusion (dbt 2.x), `relation.type` is a plain string and there is no `.render()`,
    so we provide this macro and call it instead. To stay compatible with both
    runtimes from the same macro body, dispatch on `dbt_version` and call the
    underlying `.render_for_alter()` method when running under dbt-core (1.x). -#}
{% macro render_type(relation_type) -%}
  {% if dbt_version.startswith('2.') %}
    {% if relation_type == 'metric_view' %}
      VIEW
    {% else %}
      {{- relation_type | replace("_", " ") | upper -}}
    {% endif %}
  {% else %}
    {{- relation_type.render_for_alter() -}}
  {% endif %}
{%- endmacro %}
