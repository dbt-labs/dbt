{% macro use_materialization_v2() %}
  {#-- DIVERGENCE BEGIN: v2 adapter doesn't expose get_behavior_flag_no_warn as a Jinja-callable method; use the direct attribute-access form, which fs supports and is functionally equivalent. --#}
  {{- return(config.get('use_materialization_v2', adapter.behavior.use_materialization_v2)) -}}
  {#-- DIVERGENCE END --#}
{% endmacro %}
