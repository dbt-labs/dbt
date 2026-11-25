{% macro optimize(relation) %}
  {{ return(adapter.dispatch('optimize', 'dbt')(relation)) }}
{% endmacro %}

{%- macro databricks__optimize(relation) -%}
  {%- set skip_optimize = config.get('skip_optimize', none) -%}
  {%- if skip_optimize is not none and skip_optimize | as_bool -%}
  {%- elif var('DATABRICKS_SKIP_OPTIMIZE', 'false')|lower != 'true' and
        var('databricks_skip_optimize', 'false')|lower != 'true' and
        adapter.resolve_file_format(config) == 'delta' -%}
    {%- set zorder = config.get('zorder', False) -%}
    {%- set liquid_clustered_by = config.get('liquid_clustered_by', False) -%}
    {%- set auto_liquid_cluster = config.get('auto_liquid_cluster', False) -%}
    {%- if zorder or liquid_clustered_by or auto_liquid_cluster -%}
      {#- Predictive Optimization clusters managed liquid tables itself; an explicit skip_optimize=false opts back in. -#}
      {%- set left_to_predictive_optimization = skip_optimize is none and (
            (liquid_clustered_by and is_predictive_optimization_enabled(relation))
            or (auto_liquid_cluster and not liquid_clustered_by)
          ) -%}
      {%- if not left_to_predictive_optimization -%}
        {%- call statement('run_optimize_stmt') -%}
          {{ get_optimize_sql(relation) }}
        {%- endcall -%}
      {%- elif zorder -%}
        {{ exceptions.warn("Both zorder and liquid_clustering are set but they are incompatible. zorder will be ignored.") }}
      {%- endif -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}

{%- macro get_optimize_sql(relation) %}
  optimize {{ relation.render() }}
  {%- if config.get('zorder', False) and adapter.resolve_file_format(config) == 'delta' %}
    {%- if config.get('liquid_clustered_by', False) or config.get('auto_liquid_cluster', False) %}
      {{ exceptions.warn("Both zorder and liquid_clustering are set but they are incompatible. zorder will be ignored.") }}
    {%- else %}
      {%- set zorder = config.get('zorder', none) %}
      {# TODO: predicates here? WHERE ...  #}
      {%- if zorder is sequence and zorder is not string %}
        zorder by (
        {%- for col in zorder %}
        {{ col }}{% if not loop.last %}, {% endif %}
        {%- endfor %}
        )
      {%- else %}
        zorder by ({{zorder}})
      {%- endif %}
    {%- endif %}
  {%- endif %}
{%- endmacro -%}
