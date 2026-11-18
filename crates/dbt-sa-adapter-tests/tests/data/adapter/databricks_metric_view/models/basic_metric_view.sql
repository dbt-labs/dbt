{{ config(materialized='metric_view') }}

version: 0.1
source: "{{ ref('source_orders') }}"
dimensions:
  - name: order_date
    expr: order_date
  - name: status
    expr: status
measures:
  - name: total_orders
    expr: count(1)
  - name: total_revenue
    expr: sum(revenue)
