happy_macros_sql = """
{% macro no_args() %}
  {% if execute %}
    {% call statement(auto_begin=True) %}
      create table "{{ schema }}"."no_args" (id int);
      commit;
    {% endcall %}
  {% endif %}
{% endmacro %}


{% macro table_name_args(table_name) %}
  {% if execute %}
    {% call statement(auto_begin=True) %}
      create table "{{ schema }}"."{{ table_name }}" (id int);
      commit;
    {% endcall %}
  {% endif %}
{% endmacro %}

{% macro select_something(name) %}
  {% set query %}
    select 'hello, {{ name }}' as name
  {% endset %}
  {% set table = run_query(query) %}

  {% if table.columns['name'][0] != 'hello, world' %}
    {% do exceptions.raise_compiler_error("unexpected result: " ~ table) %}
  {% endif %}
{% endmacro %}

{% macro vacuum(table_name) %}
  {% set query %}
    vacuum "{{ schema }}"."{{ table_name }}"
  {% endset %}
  {% do run_query(query) %}
{% endmacro %}


{% macro vacuum_ref(ref_target) %}
  {% set query %}
    vacuum {{ ref(ref_target) }}
  {% endset %}
  {% do run_query(query) %}
{% endmacro %}


{% macro log_graph() %}
  {% for node in graph.nodes.values() %}
    {{ log((node | string), info=True)}}
  {% endfor %}
{% endmacro %}


{% macro print_something() %}
  {{ print("You're doing awesome!") }}
{% endmacro %}
"""

sad_macros_sql = """
{% macro syntax_error() %}
  {% if execute %}
    {% call statement() %}
        select NOPE NOT A VALID QUERY
    {% endcall %}
  {% endif %}
{% endmacro %}
"""

model_sql = """
select 1 as id
"""

private_model_sql = """
select 1 as id
"""

groups_yml = """
groups:
  - name: analytics
    owner:
      name: analytics_owner
"""

private_model_schema_yml = """
models:
  - name: private_model
    access: private
    group: analytics
"""

ref_private_model_macro_sql = """
{% macro ref_private_model() %}
  {% set result = run_query("select * from " ~ ref('private_model')) %}
  {{ log("Successfully referenced private model", info=true) }}
{% endmacro %}
"""

transaction_probe_macros_sql = """
{% macro probe_insert(tag) %}
  {% call statement('probe') %}
    insert into {{ target.schema }}.txn_probe values ('{{ tag }}')
  {% endcall %}
{% endmacro %}

{% macro probe_insert_run_query(tag) %}
  {% do run_query("insert into " ~ target.schema ~ ".txn_probe values ('" ~ tag ~ "')") %}
{% endmacro %}

{% macro log_only() %}
  {{ log("nothing to commit", info=true) }}
{% endmacro %}
"""
