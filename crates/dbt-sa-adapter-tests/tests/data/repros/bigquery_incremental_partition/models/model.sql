{{
    config(
        materialized='incremental',
        incremental_strategy='insert_overwrite',
        partition_by={
            "copy_partitions": true,
            "field": "created_at",
            "data_type": "date",
            "granularity": "day",
            "time_ingestion_partitioning": true,
        },
        unique_key='id',
    )
}}

SELECT
    id,
    name,
    species,
    age,
    created_at
FROM {{ source('raw', 'animal') }}