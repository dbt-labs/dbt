-- funcsign: (relation) -> string
{%- macro get_rename_intermediate_sql(relation) -%}
    {{- log('Applying RENAME INTERMEDIATE to: ' ~ relation) -}}
    {{- adapter.dispatch('get_rename_intermediate_sql', 'dbt')(relation) -}}
{%- endmacro -%}


-- funcsign: (relation) -> string
{%- macro default__get_rename_intermediate_sql(relation) -%}

    -- get the standard intermediate name
    {% set intermediate_relation = make_intermediate_relation(relation) %}

    {{ get_rename_sql(intermediate_relation, relation.identifier) }}

{%- endmacro -%}
