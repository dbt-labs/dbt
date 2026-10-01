{% macro databricks__comment_clause() %}
  {%- set raw_persist_docs = config.get('persist_docs', {}) -%}
  {%- if raw_persist_docs is mapping -%}
    {%- set raw_relation = raw_persist_docs.get('relation', false) -%}
      {%- if raw_relation and model.description -%}
      comment '{{ model.description | replace("'", "\\'") }}'
      {%- endif -%}
  {%- elif raw_persist_docs -%}
    {{ exceptions.raise_compiler_error("Invalid value provided for 'persist_docs'. Expected dict but got value: " ~ raw_persist_docs) }}
  {% endif %}
{%- endmacro -%}


