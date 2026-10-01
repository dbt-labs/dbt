{%- macro snowflake__get_rename_intermediate_sql(relation) -%}

    -- get the standard intermediate name
    {% set intermediate_relation = make_intermediate_relation(relation) %}

    -- use `render` to ensure that the fully qualified name is used
    {{ get_rename_sql(intermediate_relation, relation.render()) }}

{%- endmacro -%}
