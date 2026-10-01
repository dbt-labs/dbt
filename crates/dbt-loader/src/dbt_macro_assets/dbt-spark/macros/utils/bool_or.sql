{#-- Spark v3 supports 'bool_or' and 'any', but Spark v2 needs to use 'max' for this
  -- https://spark.apache.org/docs/latest/api/sql/index.html#any
  -- https://spark.apache.org/docs/latest/api/sql/index.html#bool_or
  -- https://spark.apache.org/docs/latest/api/sql/index.html#max
#}

{% macro spark__bool_or(expression) -%}

    max({{ expression }})

{%- endmacro %}
