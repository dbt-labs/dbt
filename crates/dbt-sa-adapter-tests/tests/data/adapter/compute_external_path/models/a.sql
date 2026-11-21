{{
    config(
        materialized='table',
        location_root='silver/curated/data',
    )
}}

{% if execute -%}

    {%- set model_path = adapter.compute_external_path(config, model, is_incremental()) %}
    {{ print("compute_external_path: " ~ model_path) }}

{%- endif %}

select 1 as id