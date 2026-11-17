SELECT column_name, data_type, is_nullable
    FROM SVV_COLUMNS
    WHERE table_catalog = 'ci'
    AND table_schema = 'adapters_redshift_materialized_view_config_changeset'
    AND table_name = 'source'