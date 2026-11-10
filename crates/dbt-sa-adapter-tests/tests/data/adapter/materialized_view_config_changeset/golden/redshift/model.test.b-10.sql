SELECT
            a.attname as column,
            a.attisdistkey as is_dist_key,
            a.attsortkeyord as sort_key_position
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        WHERE
            n.nspname ilike 'adapters_redshift_materialized_view_config_changeset'
            AND c.relname LIKE 'mv_tbl__b__%'