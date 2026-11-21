
        create materialized view ci.adapters_redshift_materialized_view_config_changeset.a
        backup yes
        diststyle even
        
        
        auto refresh no
    as (
        -- deviation from core: use rendered SQL directly instead of raw value from RedshiftMaterializedViewConfig
        

select * from "ci"."adapters_redshift_materialized_view_config_changeset"."source"
    )

