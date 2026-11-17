{#- dbt's cross-database macros. The driver handles dbt-core's defaults for
    dateadd, datediff, last_day, date_spine, listagg, bool_or and any_value
    natively; these two need help. -#}

{#- dbt's default safe_cast is a plain CAST, which raises on a bad value.
    The driver has TRY_CAST, which returns NULL instead. -#}
{% macro redis_adbc__safe_cast(field, type) -%}
  try_cast({{ field }} as {{ type }})
{%- endmacro %}

{#- dbt's default listagg with limit_num slices an array; the driver has no
    arrays. Without a limit, dbt's default (LISTAGG … WITHIN GROUP) works. -#}
{% macro redis_adbc__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
  {%- if limit_num -%}
    {{ exceptions.raise_compiler_error("dbt.listagg with limit_num isn't supported on redis_adbc: it needs arrays, which the Redis ADBC driver doesn't have. Filter the rows first, or drop the limit.") }}
  {%- endif -%}
  {{ return(default__listagg(measure, delimiter_text, order_by_clause, limit_num)) }}
{%- endmacro %}
