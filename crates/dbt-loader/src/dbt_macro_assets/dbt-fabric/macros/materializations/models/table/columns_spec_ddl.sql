{% macro build_columns_constraints(relation) %}
    {{ return(adapter.dispatch('build_columns_constraints', 'dbt')(relation)) }}
{% endmacro %}

{% macro fabric__build_columns_constraints(relation) %}
  {# loop through user_provided_columns to create DDL with data types and constraints #}
    {%- set raw_column_constraints = adapter.render_raw_columns_constraints(raw_columns=model['columns']) -%}
    (
      {% for c in raw_column_constraints -%}
        {{ c }}{{ "," if not loop.last }}
      {% endfor %}
    )
{% endmacro %}

{% macro build_model_constraints(relation) %}
    {{ return(adapter.dispatch('build_model_constraints', 'dbt')(relation)) }}
{% endmacro %}

{% macro fabric__build_model_constraints(relation) %}
  {# loop through user_provided_columns to create DDL with data types and constraints #}
    {%- set raw_model_constraints = adapter.render_raw_model_constraints(raw_constraints=model['constraints']) -%}
    {% for c in raw_model_constraints -%}
      {% set alter_table_script %}
        alter table {{ relation.include(database=False) }} {{c}};
      {%endset%}
      {% call statement('alter_table_add_constraint') -%}
        {{alter_table_script}}
      {%- endcall %}
    {% endfor -%}
{% endmacro %}
