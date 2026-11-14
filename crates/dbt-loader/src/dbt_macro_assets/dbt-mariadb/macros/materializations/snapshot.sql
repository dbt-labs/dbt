{% macro mariadb__snapshot_string_as_time(timestamp) -%}
    {%- set result = "cast('" ~ timestamp ~ "' as datetime(6))" -%}
    {{ return(result) }}
{%- endmacro %}


{% macro mariadb__snapshot_merge_sql(target, source, insert_cols) -%}
    {#- MariaDB has no MERGE: close changed rows with an UPDATE ... JOIN, then
        insert the new versions. Fusion splits the two statements on ';'. -#}
    {%- set insert_cols_csv = insert_cols | join(', ') -%}
    {%- set columns = config.get("snapshot_table_column_names") or get_snapshot_table_column_names() -%}
    {%- set dbt_valid_to_current = config.get('dbt_valid_to_current') -%}

    update {{ target.render() }} as DBT_INTERNAL_DEST
    join {{ source }} as DBT_INTERNAL_SOURCE
      on DBT_INTERNAL_SOURCE.{{ columns.dbt_scd_id }} = DBT_INTERNAL_DEST.{{ columns.dbt_scd_id }}
    set DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} = DBT_INTERNAL_SOURCE.{{ columns.dbt_valid_to }}
    where (
        {%- if dbt_valid_to_current %}
            DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} = {{ dbt_valid_to_current }}
            or DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
        {%- else %}
            DBT_INTERNAL_DEST.{{ columns.dbt_valid_to }} is null
        {%- endif %}
    )
    and DBT_INTERNAL_SOURCE.dbt_change_type in ('update', 'delete');

    insert into {{ target.render() }} ({{ insert_cols_csv }})
    select {{ insert_cols_csv }}
    from {{ source }} as DBT_INTERNAL_SOURCE
    where DBT_INTERNAL_SOURCE.dbt_change_type = 'insert'
{%- endmacro %}



{#- MariaDB only reads `--` as a comment when whitespace follows it. The
    default staging SQL has `--avoid updating the record's valid_to ...`,
    where the quote then opens a string literal. -#}
{% macro mariadb__snapshot_staging_table(strategy, source_sql, target_relation) -%}
    {{ default__snapshot_staging_table(strategy, source_sql, target_relation) | replace('--avoid', '-- avoid') }}
{%- endmacro %}
