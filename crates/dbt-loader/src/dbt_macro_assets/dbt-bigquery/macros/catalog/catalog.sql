{# DIVERGENCE:
    parameter `information_schema` is now `dbschema`,
    the diffs are due to in Fusion, we do not have the InformationSchema objects implemented the same way as in the Python adapter.
    But the rendered SQL should be the same.

    NOTE: no `dbt_version` guard needed — these `_bigquery__*` catalog helpers are only
    invoked from other Fusion-diverged callers in this same crate (by_schema.sql / by_relation.sql).
    They are not reached by dbt-core (1.x), which runs the upstream catalog flow.
#}
{% macro _bigquery__get_table_shards_sql(db_schema) %}
    {% set region = "region-%s"|format(db_schema.identifier)%}
    select
        info_tables.table_catalog as table_catalog,
        info_tables.table_schema as table_schema,
        coalesce(REGEXP_EXTRACT(info_tables.table_name, '^(.+)[0-9]{8}$'), info_tables.table_name) as table_name,
        info_tables.table_name as shard_name,
        REGEXP_EXTRACT(info_tables.table_name, '^.+([0-9]{8})$') as shard_index,
        REGEXP_CONTAINS(info_tables.table_name, '^.+[0-9]{8}$') and  info_tables.table_type = 'BASE TABLE' as is_date_shard,
        info_tables.table_type as table_type,
        info_tables.table_type = 'BASE TABLE' as is_table,
        JSON_VALUE(table_description.option_value) as table_comment,
        table_storage.total_logical_bytes as size_bytes,
        table_storage.total_rows as row_count
    from `{{ db_schema.database }}`.`{{ db_schema.schema }}`.INFORMATION_SCHEMA.TABLES info_tables
    left join `{{ db_schema.database }}`.`{{ db_schema.schema }}`.INFORMATION_SCHEMA.MATERIALIZED_VIEWS materialized_views
        on materialized_views.table_catalog = info_tables.table_catalog
        and materialized_views.table_schema = info_tables.table_schema
        and materialized_views.table_name = info_tables.table_name
    left join `{{ db_schema.database }}`.`{{ db_schema.schema }}`.INFORMATION_SCHEMA.TABLE_OPTIONS table_description
        on table_description.table_catalog = info_tables.table_catalog
        and table_description.table_schema = info_tables.table_schema
        and table_description.table_name = info_tables.table_name
        and table_description.option_name = 'description'
    left join `{{ db_schema.database }}`.`{{ region }}`.INFORMATION_SCHEMA.TABLE_STORAGE table_storage
        on table_storage.table_catalog = info_tables.table_catalog
        and table_storage.table_schema = info_tables.table_schema
        and table_storage.table_name = info_tables.table_name
{% endmacro %}


{% macro _bigquery__get_tables_sql() %}
    select distinct
        table_catalog,
        table_schema,
        table_name,
        is_date_shard,
        table_type,
        is_table,
        table_comment
    from table_shards
{% endmacro %}


{% macro _bigquery__get_table_stats_sql() %}
    select
        table_catalog,
        table_schema,
        table_name,
        max(shard_name) as latest_shard_name,
        min(shard_index) as shard_min,
        max(shard_index) as shard_max,
        count(shard_index) as shard_count,
        sum(size_bytes) as size_bytes,
        sum(row_count) as row_count
    from table_shards
    group by 1, 2, 3
{% endmacro %}


{# DIVERGENCE: parameter `information_schema` is now `dbschema` #}
{% macro _bigquery__get_columns_sql(db_schema) %}
    select
        columns.table_catalog,
        columns.table_schema,
        columns.table_name as shard_name,
        coalesce(paths.field_path, '<unknown>') as column_name,
        -- invent a row number to account for nested fields
        -- BQ does not treat these nested properties as independent fields
        row_number() over (
            partition by
                columns.table_catalog,
                columns.table_schema,
                columns.table_name
            order by
                columns.ordinal_position,
                paths.field_path
        ) as column_index,
        coalesce(paths.data_type, '<unknown>') as column_type,
        paths.description as column_comment,
        case when columns.is_partitioning_column = 'YES' then 1 else 0 end as is_partitioning_column,
        case when columns.is_partitioning_column = 'YES' then paths.field_path end as partition_column,
        case when columns.clustering_ordinal_position is not null then 1 else 0 end as is_clustering_column,
        case when columns.clustering_ordinal_position is not null then paths.field_path end as cluster_column,
        columns.clustering_ordinal_position
    {# DIVERGENCE BEGIN #}
    from `{{ db_schema.database }}`.`{{ db_schema.schema }}`.INFORMATION_SCHEMA.COLUMNS columns
    join `{{ db_schema.database }}`.`{{ db_schema.schema }}`.INFORMATION_SCHEMA.COLUMN_FIELD_PATHS paths
    {# DIVERGENCE END #}
        on paths.table_catalog = columns.table_catalog
        and paths.table_schema = columns.table_schema
        and paths.table_name = columns.table_name
        and paths.column_name = columns.column_name
    where columns.ordinal_position is not null
{% endmacro %}


{% macro _bigquery__get_column_stats_sql() %}
    select
        table_catalog,
        table_schema,
        shard_name,
        max(is_partitioning_column) = 1 as is_partitioned,
        max(partition_column) as partition_column,
        max(is_clustering_column) = 1 as is_clustered,
        array_to_string(
            array_agg(
                cluster_column ignore nulls
                order by clustering_ordinal_position
            ), ', '
        ) as clustering_columns
    from columns
    group by 1, 2, 3
{% endmacro %}


{% macro _bigquery__get_extended_catalog_sql() %}
    select
        tables.table_catalog as table_database,
        tables.table_schema,
        case
            when tables.is_date_shard then concat(tables.table_name, '*')
            else tables.table_name
        end as table_name,
        tables.table_type,
        tables.table_comment,
        -- coalesce column metadata fields to ensure they are non-null for catalog generation
        -- external table columns are not present in COLUMN_FIELD_PATHS
        coalesce(columns.column_name, '<unknown>') as column_name,
        coalesce(columns.column_index, 1) as column_index,
        coalesce(columns.column_type, '<unknown>') as column_type,
        coalesce(columns.column_comment, '') as column_comment,

        'Shard count' as `stats__date_shards__label`,
        table_stats.shard_count as `stats__date_shards__value`,
        'The number of date shards in this table' as `stats__date_shards__description`,
        tables.is_date_shard as `stats__date_shards__include`,

        'Shard (min)' as `stats__date_shard_min__label`,
        table_stats.shard_min as `stats__date_shard_min__value`,
        'The first date shard in this table' as `stats__date_shard_min__description`,
        tables.is_date_shard as `stats__date_shard_min__include`,

        'Shard (max)' as `stats__date_shard_max__label`,
        table_stats.shard_max as `stats__date_shard_max__value`,
        'The last date shard in this table' as `stats__date_shard_max__description`,
        tables.is_date_shard as `stats__date_shard_max__include`,

        '# Rows' as `stats__num_rows__label`,
        table_stats.row_count as `stats__num_rows__value`,
        'Approximate count of rows in this table' as `stats__num_rows__description`,
        tables.is_table as `stats__num_rows__include`,

        'Approximate Size' as `stats__num_bytes__label`,
        table_stats.size_bytes as `stats__num_bytes__value`,
        'Approximate size of table as reported by BigQuery' as `stats__num_bytes__description`,
        tables.is_table as `stats__num_bytes__include`,

        'Partitioned By' as `stats__partitioning_type__label`,
        column_stats.partition_column as `stats__partitioning_type__value`,
        'The partitioning column for this table' as `stats__partitioning_type__description`,
        column_stats.is_partitioned as `stats__partitioning_type__include`,

        'Clustered By' as `stats__clustering_fields__label`,
        column_stats.clustering_columns as `stats__clustering_fields__value`,
        'The clustering columns for this table' as `stats__clustering_fields__description`,
        column_stats.is_clustered as `stats__clustering_fields__include`

    from tables
    join table_stats
        on table_stats.table_catalog = tables.table_catalog
        and table_stats.table_schema = tables.table_schema
        and table_stats.table_name = tables.table_name
    left join column_stats
        on column_stats.table_catalog = tables.table_catalog
        and column_stats.table_schema = tables.table_schema
        and column_stats.shard_name = table_stats.latest_shard_name
    left join columns
        on columns.table_catalog = tables.table_catalog
        and columns.table_schema = tables.table_schema
        and columns.shard_name = table_stats.latest_shard_name
{% endmacro %}
