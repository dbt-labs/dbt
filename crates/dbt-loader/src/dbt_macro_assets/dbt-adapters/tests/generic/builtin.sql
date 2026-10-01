/* {#
  Generic tests can be defined in `macros/` or in `tests/generic`.
  These four tests are built into the dbt-core global project.
  To support extensibility to other adapters and SQL dialects,
  they call 'dispatched' macros. By default, they will use
  the SQL defined in `global_project/macros/generic_test_sql`
#} */

{% test unique(model, column_name) %}
    {% set macro = adapter.dispatch('test_unique', 'dbt') %}
    {{ macro(model, column_name) }}
{% endtest %}


{% test not_null(model, column_name) %}
    {% set macro = adapter.dispatch('test_not_null', 'dbt') %}
    {{ macro(model, column_name) }}
{% endtest %}


{% test accepted_values(model, column_name, values, quote=True) %}
    {% set macro = adapter.dispatch('test_accepted_values', 'dbt') %}
    {{ macro(model, column_name, values, quote) }}
{% endtest %}


{% test relationships(model, column_name, to, field) %}
    {% set macro = adapter.dispatch('test_relationships', 'dbt') %}
    {{ macro(model, column_name, to, field) }}
{% endtest %}
