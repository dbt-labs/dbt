{{
  config(
    materialized='metric_view',
    databricks_tags={
      'team': 'analytics',
      'environment': 'test'
    }
  )
}}

version: 0.1
source: "{{ ref('source_orders') }}"
dimensions:
  - name: status
    expr: status
measures:
  - name: order_count
    expr: count(1)
