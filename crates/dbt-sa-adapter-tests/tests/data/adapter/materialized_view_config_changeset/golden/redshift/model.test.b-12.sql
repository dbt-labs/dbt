-- apply a full refresh immediately if needed
    alter materialized view "ci"."adapters_redshift_materialized_view_config_changeset"."b"
            -- intentional divergence: autorefresh is already a bool
            auto refresh no