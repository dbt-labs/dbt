{{
    config(
        materialized = 'incremental',
        incremental_strategy = 'microbatch',
        event_time = 'event_ts',
        begin = '2025-01-01',
        batch_size = 'day'
    )
}}

select event_ts, id from {{ ref('source_events') }}
