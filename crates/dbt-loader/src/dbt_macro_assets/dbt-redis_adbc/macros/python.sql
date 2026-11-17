{#- Python models run in the dbt process (submit_python_job in impl.py).
    The code it runs is what dbt compiled (the model's code, then dbt's
    py_script_postfix), then the relation to write model()'s result to. The
    model's code starts on the first line, so tracebacks give its line
    numbers. Tables only: dbt's incremental materialization runs SQL models
    only. -#}
{% macro redis_adbc__py_write_table(temporary, relation, compiled_code) -%}
  {%- if temporary -%}
    {{ exceptions.raise_compiler_error("redis_adbc builds Python models as tables only (" ~ relation ~ ")") }}
  {%- endif -%}
{{ compiled_code }}


# dbt-redis-adbc ingests model()'s result into this table.
dbt_redis_adbc_target = {"schema": {{ relation.schema | tojson }}, "identifier": {{ relation.identifier | tojson }}}
{%- endmacro %}
