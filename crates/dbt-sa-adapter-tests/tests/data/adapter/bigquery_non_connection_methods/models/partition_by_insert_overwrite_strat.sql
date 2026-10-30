{{
    config(
        materialized='incremental',
        partition_by={
            "field": "created_at",
            "data_type": "timestamp",
            "granularity": "day"
        },
        incremental_strategy='insert_overwrite',
    )
}}

{% set partition_by = config.get('partition_by', none) %}

{%- if execute: -%}
  {{ print("raw object partition_by: " ~ partition_by) }}
  {% set parsed_partition_by = adapter.parse_partition_by(partition_by) %}
  {{ print("parsed object partition_by: " ~ parsed_partition_by) }}

  {% set partition_by_render = parsed_partition_by.render() %}
  {{ print("partition by render: " ~ partition_by_render) }}

  {# Log all available values from BigqueryPartitionConfig Object implementation #}
  {{ print("=== BigqueryPartitionConfig Values ===") }}
  {{ print("field: " ~ parsed_partition_by.field) }}
  {{ print("data_type: " ~ parsed_partition_by.data_type) }}
  {{ print("granularity: " ~ parsed_partition_by.granularity) }}
  {{ print("time_ingestion_partitioning: " ~ parsed_partition_by.time_ingestion_partitioning) }}
  {{ print("copy_partitions: " ~ parsed_partition_by.copy_partitions) }}

  {# Log all available methods from BigqueryPartitionConfig Object implementation #}
  {{ print("=== BigqueryPartitionConfig Methods ===") }}
  {{ print("data_type_for_partition(): " ~ parsed_partition_by.data_type_for_partition()) }}
  {{ print("time_partitioning_field(): " ~ parsed_partition_by.time_partitioning_field()) }}
  {{ print("render_wrapped(): " ~ parsed_partition_by.render_wrapped()) }}
  {{ print("render_wrapped(alias='t'): " ~ parsed_partition_by.render_wrapped(alias='t')) }}
{%- endif -%}

select 42 as my_lucky_number
