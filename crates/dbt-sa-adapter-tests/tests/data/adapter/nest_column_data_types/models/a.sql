{# somehow get this from the YAML user docs #}

{% set columns  = model.columns %}
{{ log('columns', info=True) }}
{{ print(columns) }}

{% set columns_nested = adapter.nest_column_data_types(columns) %}
{{ log('columns_nested', info=True) }}
{{ print(columns_nested) }}

select 1 as id;