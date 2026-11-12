select
            tb.database,
            tb.schema,
            tb.table,
            tb.diststyle,
            tb.sortkey1,
            mv.autorefresh
        from svv_table_info tb
        -- svv_mv_info is queryable by Redshift Serverless, but stv_mv_info is not
        left join svv_mv_info mv
            on mv.database_name = tb.database
            and mv.schema_name = tb.schema
            and mv.name = tb.table
        where tb.table ilike 'a'
        and tb.schema ilike 'adapters_redshift_materialized_view_config_changeset'
        and tb.database ilike 'ci'