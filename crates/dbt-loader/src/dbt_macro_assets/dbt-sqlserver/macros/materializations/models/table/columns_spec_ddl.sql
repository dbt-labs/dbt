{% macro build_columns_constraints(relation) %}
    {{ return(adapter.dispatch('build_columns_constraints', 'dbt')(relation)) }}
{% endmacro %}

{% macro sqlserver__build_columns_constraints(relation) %}
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

{% macro sqlserver__build_model_constraints(relation) %}
  {#- The shared renderer yields `[constraint <name> ]<body>`, which needs ADD in
      front to be T-SQL. Only under an enforced contract, as for column constraints. -#}
    {%- set contract_config = config.get('contract') -%}
    {%- if not contract_config or not contract_config.enforced -%}
      {{ return('') }}
    {%- endif -%}
    {%- set raw_model_constraints = adapter.render_raw_model_constraints(raw_constraints=model.get('constraints') or []) -%}
    {% for c in raw_model_constraints -%}
      {% set alter_table_script %}
        {{ get_use_database_sql(relation.database) }}
        alter table {{ relation.include(database=False) }} add {{c}};
      {%endset%}
      {% call statement('alter_table_add_constraint') -%}
        {{alter_table_script}}
      {%- endcall %}
    {% endfor -%}
{% endmacro %}
