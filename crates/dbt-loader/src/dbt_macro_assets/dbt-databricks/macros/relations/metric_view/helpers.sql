{#-
  Wrap backtick-rendered SQL identifiers on metric_view `source:` lines in YAML double
  quotes so the YAML parser does not choke on them. Other keys are left untouched.
  Reference: https://github.com/databricks/dbt-databricks/blob/2c3aa9fdddbab30a3c4a660c5e98722e989a592b/dbt/adapters/databricks/handle.py#L374
-#}
{% macro databricks__yaml_quote_backtick_values(yaml_body) %}
  {%- if '`' not in yaml_body -%}
    {{- return(yaml_body) -}}
  {%- endif -%}
  {%- set pattern = '^( *source *: +)(`[^`\\n]+`(?:\\.`[^`\\n]+`)*)( *(?:#[^\\n]*)?)$' -%}
  {{- return(modules.re.sub(pattern, '\\1"\\2"\\3', yaml_body, flags=modules.re.MULTILINE)) -}}
{% endmacro %}
