{%- macro snowflake__get_create_backup_sql(relation) -%}

    -- get the standard backup name
    {% set backup_relation = make_backup_relation(relation, relation.type) %}

    -- drop any pre-existing backup
    {{ get_drop_sql(backup_relation) }};

    -- use `render` to ensure that the fully qualified name is used
    {{ get_rename_sql(relation, backup_relation.render()) }}

{%- endmacro -%}
