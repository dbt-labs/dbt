{{ config(materialized='metric_view') }}

version: 0.1
source: "{{ ref('source_orders') }}"
filter: status = 'completed'
dimensions:
  - name: order_date
    expr: order_date
measures:
  - name: completed_orders
    expr: count(1)
  - name: completed_revenue
    expr: sum(revenue)
