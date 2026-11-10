{% macro exasol__snapshot_hash_arguments(args) -%}
    {#- default__snapshot_hash_arguments emits md5(cast(x as varchar)): Exasol has
        no md5() (the builtin is HASH_MD5, returning CHAR(32) hex like dbt's md5
        surrogate keys) and rejects an unsized VARCHAR ("expecting '('"). VARCHAR is
        unpadded so the width doesn't change the hash; 2,000,000 (the max) avoids
        truncating a long composite key before hashing, matching exasol__hash. The
        '|' delimiter keeps argument boundaries distinct. -#}
    hash_md5(
        {%- for arg in args -%}
            coalesce(cast({{ arg }} as varchar(2000000)), '')
            {%- if not loop.last %} || '|' || {% endif -%}
        {%- endfor -%}
    )
{%- endmacro %}


{% macro exasol__snapshot_string_as_time(timestamp) -%}
    {#- default raises not_implemented. Exasol TO_TIMESTAMP parses the stringified
        snapshot time back into a TIMESTAMP. -#}
    {%- set result = "to_timestamp('" ~ timestamp ~ "')" -%}
    {{ return(result) }}
{%- endmacro %}


{% macro exasol__snapshot_merge_sql(target, source, insert_cols) -%}
    {#- default__snapshot_merge_sql uses `when matched and <cond> then update` and
        `when not matched and <cond> then insert`. Exasol's MERGE puts the condition
        in a trailing WHERE on the UPDATE/INSERT clause instead (the ON clause only
        permits equality), so the per-clause predicate is moved into WHERE. -#}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}
    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}
    {%- set dbt_valid_to_current = config.get('dbt_valid_to_current') -%}

    merge into {{ target.render() }} as DBT_INTERNAL_DEST
    using {{ source }} as DBT_INTERNAL_SOURCE
    on DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }} = DBT_INTERNAL_DEST.{{ columns.dbt_scd_id }}

    when matched then update
        set {{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
        where (
            {%- if dbt_valid_to_current %}
                DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} = {{ snapshot_string_as_time(dbt_valid_to_current) }}
                or DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
            {%- else %}
                DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
            {%- endif %}
        )
        and DBT_INTERNAL_SOURCE.dbt_change_type in ('update', 'delete')

    when not matched then insert ({{ insert_cols_csv }})
        values ({{ insert_cols_csv }})
        where DBT_INTERNAL_SOURCE.dbt_change_type = 'insert'
{%- endmacro %}
