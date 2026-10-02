metricflow_time_spine_sql = """
SELECT to_date('02/20/2023', 'mm/dd/yyyy') as date_day, 'fq' as fiscal_quarter
"""

customers_sql = """
select 1 as customer_id, 'JP' as country, current_timestamp as signed_up_at
"""

orders_sql = """
select 1 as order_id, 1 as customer_id, 'open' as status, current_timestamp as ordered_at
"""

time_spine_with_custom_granularity_yml = """version: 2

models:
  - name: metricflow_time_spine
    time_spine:
      standard_granularity_column: date_day
      custom_granularities:
        - name: fiscal_quarter
    columns:
      - name: date_day
        granularity: day
      - name: fiscal_quarter
"""

semantic_models_yml = """version: 2

semantic_models:
  - name: customers_sm
    model: ref('customers')
    dimensions:
      - name: country
        type: categorical
      - name: signed_up_at
        type: time
        type_params:
          time_granularity: day
    measures:
      - name: customer_count
        agg: count
        expr: customer_id
    entities:
      - name: customer
        type: primary
        expr: customer_id
    defaults:
      agg_time_dimension: signed_up_at

  - name: orders_sm
    model: ref('orders')
    dimensions:
      - name: status
        type: categorical
      - name: ordered_at
        type: time
        type_params:
          time_granularity: day
    measures:
      - name: order_count
        agg: count
        expr: order_id
    entities:
      - name: order
        type: primary
        expr: order_id
      - name: customer
        type: foreign
        expr: customer_id
    defaults:
      agg_time_dimension: ordered_at
"""


def exposure_yml(*specifiers: str) -> str:
    depends_on = "\n".join(f"      - dimension('{specifier}')" for specifier in specifiers)
    return f"""version: 2

exposures:
  - name: dashboard
    type: dashboard
    owner:
      name: Dashboard Owner
    depends_on:
{depends_on}
"""


v2_inline_schema_yml = """version: 2

models:
  - name: customers
    semantic_model: true
    agg_time_dimension: signed_up_at
    columns:
      - name: customer_id
        entity:
          name: customer
          type: primary
      - name: country
        dimension:
          name: country
          type: categorical
      - name: signed_up_at
        granularity: day
        dimension:
          name: signed_up_at
          type: time
"""

osi_document_json = """{
  "version": "0.1.1",
  "semantic_model": [
    {
      "name": "osi_customers",
      "datasets": [
        {
          "name": "osi_customers",
          "source": "%(source)s",
          "primary_key": ["customer_id"],
          "fields": [
            {
              "name": "customer_id",
              "expression": {"dialects": [{"dialect": "ANSI_SQL", "expression": "customer_id"}]}
            },
            {
              "name": "country",
              "expression": {"dialects": [{"dialect": "ANSI_SQL", "expression": "country"}]}
            },
            {
              "name": "signed_up_at",
              "expression": {"dialects": [{"dialect": "ANSI_SQL", "expression": "signed_up_at"}]},
              "dimension": {"is_time": true}
            }
          ]
        }
      ]
    }
  ]
}
"""
