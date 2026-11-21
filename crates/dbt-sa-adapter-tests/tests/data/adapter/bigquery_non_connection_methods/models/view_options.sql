{{ config(
    materialized='table',
    hours_to_expiration=7,
    labels={
      "label1": "1"
    },
    partition_by={
      "field": "created_at",
    },
    labels_from_meta=true,
    meta={
        "field_1": "updated_at",
        "field": "created_at_from_meta"
    }
)}}

{% set view_options = adapter.get_view_options(config, model) %}
{{ print(view_options) }}

{% set table_options = adapter.get_table_options(config, model, true) %}
{{ print(table_options) }}

{% set table_options = adapter.get_table_options(config, model, false) %}
{{ print(table_options) }}

select 1 as id;