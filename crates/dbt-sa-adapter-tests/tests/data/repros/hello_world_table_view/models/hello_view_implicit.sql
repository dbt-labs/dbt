{{ config(
    post_hook=[
        "insert into {{ this }} (message) values ('foo')"
    ]
)}}

--  this is expected raise exceptions.raise_compiler_error
{% if execute %}
    {{ check_model_is_table(model) }}
{% endif %}

select 'Hello World' as message
