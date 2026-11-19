{{ config(materialized='incremental', incremental_strategy='insert_overwrite', on_schema_change='append_new_columns', contract={'enforced': true}) }}

select
    1 as id,
    2 as customer,
    to_timestamp_ntz('2025-01-01 00:00:00') as ordered_at,
    30 as subtotal
