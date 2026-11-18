{{ config(
    materialized='incremental',
    unique_key='event_id',
    tags=['render-trace', target.name]
) }}

{# Common package-style setup: project vars, list construction, and dispatch. #}
{% set dimensions = var('render_trace_columns', ['account_id']) %}
{% set include_audit = var('include_audit_columns', false) %}

with source_events as (
    select *
    from {{ source('raw', 'events') }}
    {% if is_incremental() %}
    where updated_at > (select max(updated_at) from {{ this }})
    {% endif %}
),

base_values as (
    select * from {{ ref('base') }}
),

renamed as (
    select
        event_id,
        {%- for column in dimensions %}
        {{ adapter.dispatch('trace_column', 'render_trace')(column) }} as {{ column }},
        {%- endfor %}
        event_timestamp
        {%- if include_audit %},
        '{{ target.name }}' as dbt_target,
        {{ probe_value(7) }} as trace_version
        {%- endif %}
    from source_events
)

select renamed.*, base_values.base_value
from renamed
cross join base_values
