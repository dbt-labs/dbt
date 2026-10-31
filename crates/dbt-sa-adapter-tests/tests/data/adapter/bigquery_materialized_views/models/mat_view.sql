{{
    config(
        materialized="materialized_view",
        enable_refresh=true,
        refresh_interval_minutes=60.0,
        labels={"my_labels": "label_value"}
    )
}}

SELECT * FROM {{ ref("orders") }}
