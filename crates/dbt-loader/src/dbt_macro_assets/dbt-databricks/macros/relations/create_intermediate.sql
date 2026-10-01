{%- macro databricks__get_create_intermediate_sql(relation, sql) -%}
    {% set intermediate_relation = make_intermediate_relation(relation) %}

    -- drop any pre-existing intermediate
    {{ drop_relation(intermediate_relation) }}

    {{ return(get_create_sql(intermediate_relation, sql)) }}

{%- endmacro -%}