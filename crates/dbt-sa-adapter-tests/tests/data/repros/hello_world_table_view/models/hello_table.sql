{{ config(
    materialized='table',
    post_hook=[
        "insert into {{ this }} (message) values ('foo')"
    ]
)}}


{% if execute %}
    {{ check_model_is_table(model) }}
{% endif %}

select 'Hello World' as message
