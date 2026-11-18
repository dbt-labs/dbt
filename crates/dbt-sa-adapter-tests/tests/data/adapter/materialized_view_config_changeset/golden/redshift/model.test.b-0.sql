WITH materialized_views AS (
    SELECT TRIM(name) AS object_name, 'materialized_view'::text AS object_type
    FROM svv_mv_info
    WHERE TRIM(schema_name) ILIKE 'adapters_redshift_materialized_view_config_changeset'
        AND TRIM(name) ILIKE 'b'
),
all_objects AS (
    SELECT table_name AS object_name,
        CASE 
            WHEN table_type ILIKE 'BASE TABLE' THEN 'table'::text
            WHEN table_type ILIKE 'VIEW' THEN 'view'::text
            ELSE 'table'
        END AS object_type
    FROM svv_tables
    WHERE table_schema ILIKE 'adapters_redshift_materialized_view_config_changeset'
        AND table_name ILIKE 'b'
)
SELECT
    COALESCE(mv.object_name, ao.object_name) AS object_name,
    COALESCE(mv.object_type, ao.object_type) AS object_type
FROM all_objects ao
LEFT JOIN materialized_views mv
    ON ao.object_name = mv.object_name