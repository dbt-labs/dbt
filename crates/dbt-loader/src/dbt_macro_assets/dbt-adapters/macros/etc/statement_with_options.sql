{# Private: similar to statement, but allows passing options. Not recommended to use in any user defined macros use at your own risk. #}
{%- macro _statement_with_options(name=None, fetch_result=False, auto_begin=True, language='sql', options={}) -%}
  {%- if execute: -%}
    {%- set compiled_code = caller() -%}

    {%- if name == 'main' -%}
      {{ log('Writing runtime {} for node "{}"'.format(language, model['unique_id'])) }}
      {{ write(compiled_code) }}
    {%- endif -%}
    {%- if language == 'sql'-%}
      {%- set res, table = adapter.execute(compiled_code, auto_begin=auto_begin, fetch=fetch_result, options=options) -%}
    {%- elif language == 'python' -%}
      {%- set res = submit_python_job(model, compiled_code) -%}
      {#-- TODO: What should table be for python models? --#}
      {%- set table = None -%}
    {%- else -%}
      {% do exceptions.raise_compiler_error("statement macro didn't get supported language") %}
    {%- endif -%}

    {%- if name is not none -%}
      {{ store_result(name, response=res, agate_table=table) }}
    {%- endif -%}

  {%- endif -%}
{%- endmacro %}


{# Private: similar to run_query, but allows passing Arrow ADBC Statement options. Not recommended to use in any user defined macros use at your own risk. #}
{% macro _run_query_with_options(sql, options={}) %}
  {% call _statement_with_options("run_query_statement", fetch_result=true, auto_begin=false, options=options) %}
    {{ sql }}
  {% endcall %}

  {% do return(load_result("run_query_statement").table) %}
{% endmacro %}
