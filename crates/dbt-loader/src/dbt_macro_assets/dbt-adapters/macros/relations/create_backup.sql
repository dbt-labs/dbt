-- funcsign: (relation) -> string
{%- macro get_create_backup_sql(relation) -%}
    {{- log('Applying CREATE BACKUP to: ' ~ relation) -}}
    {{- adapter.dispatch('get_create_backup_sql', 'dbt')(relation) -}}
{%- endmacro -%}


-- funcsign: (relation) -> string
{%- macro default__get_create_backup_sql(relation) -%}

    -- get the standard backup name
    {% set backup_relation = make_backup_relation(relation, relation.type) %}

    -- drop any pre-existing backup
    {{ get_drop_sql(backup_relation) }};

    {{ get_rename_sql(relation, backup_relation.identifier) }}

{%- endmacro -%}
