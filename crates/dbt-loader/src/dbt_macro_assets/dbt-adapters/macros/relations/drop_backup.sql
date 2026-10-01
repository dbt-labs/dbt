-- funcsign: (relation) -> string
{%- macro get_drop_backup_sql(relation) -%}
    {{- log('Applying DROP BACKUP to: ' ~ relation) -}}
    {{- adapter.dispatch('get_drop_backup_sql', 'dbt')(relation) -}}
{%- endmacro -%}


-- funcsign: (relation) -> string
{%- macro default__get_drop_backup_sql(relation) -%}

    -- get the standard backup name
    {% set backup_relation = make_backup_relation(relation, relation.type) %}

    {{ get_drop_sql(backup_relation) }}

{%- endmacro -%}
