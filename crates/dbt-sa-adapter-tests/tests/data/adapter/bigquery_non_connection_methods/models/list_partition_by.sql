{{ config(
    materialized='table',
    hours_to_expiration="7",
    labels={
      "label1": "1"
    },
    partition_by=['load_ts']
)}}
{% set partition_by = config.get('partition_by', none) %}

{%- if execute: -%}
  {{ print("raw list partition_by: " ~ partition_by) }}
  -- this is expected to fail
  {% set parsed_partition_by = adapter.parse_partition_by(partition_by) %}
  {% do exceptions.raise_compiler_error('expected to raise, but it didn\'t. Parsed partition by: ' ~ parsed_partition_by) %}
{%- endif -%}
select 1 as load_ts;
