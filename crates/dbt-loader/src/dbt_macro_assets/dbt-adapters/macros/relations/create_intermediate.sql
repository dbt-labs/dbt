-- funcsign: (relation, string) -> string
{%- macro get_create_intermediate_sql(relation, sql) -%}
    {{- log('Applying CREATE INTERMEDIATE to: ' ~ relation) -}}
    {{- adapter.dispatch('get_create_intermediate_sql', 'dbt')(relation, sql) -}}
{%- endmacro -%}


-- funcsign: (relation, string) -> string
{%- macro default__get_create_intermediate_sql(relation, sql) -%}

    -- get the standard intermediate name
    {% set intermediate_relation = make_intermediate_relation(relation) %}

    -- drop any pre-existing intermediate
    {{ get_drop_sql(intermediate_relation) }};

    {{ get_create_sql(intermediate_relation, sql) }}

{%- endmacro -%}
