select
            vw.definition
        from pg_views vw
        where vw.viewname = 'a'
        and vw.schemaname = 'adapters_redshift_materialized_view_config_changeset'
        and vw.definition ilike '%create materialized view%'