
{% set model_constraints = model['constraints'] %}
{{ print("model_constraints") }}
{{ print(model_constraints) }}

{% set rendered = adapter.render_raw_model_constraints(raw_constraints=model_constraints) %}
{{ print("rendered_model_constraints") }}
{{ print(rendered) }}

{% set columns = model['columns'] %}
{{ print("columns") }}
{{ print(columns) }}

{% set rendered = adapter.render_raw_columns_constraints(raw_columns=columns) %}
{{ print("rendered_columns_constraints") }}
{{ print(rendered) }}

select 1 as id
