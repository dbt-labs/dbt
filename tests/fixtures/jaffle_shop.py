import os

import pytest

from dbt.tests.util import read_file

# models/customers.sql
customers_sql = """
with customers as (

    select * from {{ ref('stg_customers') }}

),

orders as (

    select * from {{ ref('stg_orders') }}

),

payments as (

    select * from {{ ref('stg_payments') }}

),

customer_orders as (

        select
        customer_id,

        min(order_date) as first_order,
        max(order_date) as most_recent_order,
        count(order_id) as number_of_orders
    from orders

    group by customer_id

),

customer_payments as (

    select
        orders.customer_id,
        sum(amount) as total_amount

    from payments

    left join orders on
         payments.order_id = orders.order_id

    group by orders.customer_id

),

final as (

    select
        customers.customer_id,
        customers.first_name,
        customers.last_name,
        customer_orders.first_order,
        customer_orders.most_recent_order,
        customer_orders.number_of_orders,
        customer_payments.total_amount as customer_lifetime_value

    from customers

    left join customer_orders
        on customers.customer_id = customer_orders.customer_id

    left join customer_payments
        on  customers.customer_id = customer_payments.customer_id

)

select * from final
"""

# models/docs.md
docs_md = """
{% docs orders_status %}

Orders can be one of the following statuses:

| status         | description                                                                                                            |
|----------------|------------------------------------------------------------------------------------------------------------------------|
| placed         | The order has been placed but has not yet left the warehouse                                                           |
| shipped        | The order has ben shipped to the customer and is currently in transit                                                  |
| completed      | The order has been received by the customer                                                                            |
| return_pending | The customer has indicated that they would like to return the order, but it has not yet been received at the warehouse |
| returned       | The order has been returned by the customer and received at the warehouse                                              |


{% enddocs %}
"""

# models/orders.sql
orders_sql = """
{% set payment_methods = ['credit_card', 'coupon', 'bank_transfer', 'gift_card'] %}

with orders as (

    select * from {{ ref('stg_orders') }}

),

payments as (

    select * from {{ ref('stg_payments') }}

),

order_payments as (

    select
        order_id,

        {% for payment_method in payment_methods -%}
        sum(case when payment_method = '{{ payment_method }}' then amount else 0 end) as {{ payment_method }}_amount,
        {% endfor -%}

        sum(amount) as total_amount

    from payments

    group by order_id

),

final as (

    select
        orders.order_id,
        orders.customer_id,
        orders.order_date,
        orders.status,

        {% for payment_method in payment_methods -%}

        order_payments.{{ payment_method }}_amount,

        {% endfor -%}

        order_payments.total_amount as amount

    from orders


    left join order_payments
        on orders.order_id = order_payments.order_id

)

select * from final
"""

# models/overview.md
overview_md = """
{% docs __overview__ %}

## Data Documentation for Jaffle Shop

`jaffle_shop` is a fictional ecommerce store.

This [dbt](https://www.getdbt.com/) project is for testing out code.

The source code can be found [here](https://github.com/clrcrl/jaffle_shop).

{% enddocs %}
"""

# models/schema.yml
schema_yml = """
version: 2

models:
  - name: customers
    description: This table has basic information about a customer, as well as some derived facts based on a customer's orders

    columns:
      - name: customer_id
        description: This is a unique identifier for a customer
        data_tests:
          - unique
          - not_null

      - name: first_name
        description: Customer's first name. PII.

      - name: last_name
        description: Customer's last name. PII.

      - name: first_order
        description: Date (UTC) of a customer's first order

      - name: most_recent_order
        description: Date (UTC) of a customer's most recent order

      - name: number_of_orders
        description: Count of the number of orders a customer has placed

      - name: customer_lifetime_value
        description: Total value (AUD) of a customer's orders

  - name: orders
    description: This table has basic information about orders, as well as some derived facts based on payments

    columns:
      - name: order_id
        data_tests:
          - unique
          - not_null
        description: This is a unique identifier for an order

      - name: customer_id
        description: Foreign key to the customers table
        data_tests:
          - not_null
          - relationships:
              to: ref('customers')
              field: customer_id

      - name: order_date
        description: Date (UTC) that the order was placed

      - name: status
        description: '{{ doc("orders_status") }}'
        data_tests:
          - accepted_values:
              values: ['placed', 'shipped', 'completed', 'return_pending', 'returned']

      - name: amount
        description: Total amount (AUD) of the order
        data_tests:
          - not_null

      - name: credit_card_amount
        description: Amount of the order (AUD) paid for by credit card
        data_tests:
          - not_null

      - name: coupon_amount
        description: Amount of the order (AUD) paid for by coupon
        data_tests:
          - not_null

      - name: bank_transfer_amount
        description: Amount of the order (AUD) paid for by bank transfer
        data_tests:
          - not_null

      - name: gift_card_amount
        description: Amount of the order (AUD) paid for by gift card
        data_tests:
          - not_null
"""

# models/staging/schema.yml
staging_schema_yml = """
version: 2

models:
  - name: stg_customers
    columns:
      - name: customer_id
        data_tests:
          - unique
          - not_null

  - name: stg_orders
    columns:
      - name: order_id
        data_tests:
          - unique
          - not_null
      - name: status
        data_tests:
          - accepted_values:
              values: ['placed', 'shipped', 'completed', 'return_pending', 'returned']

  - name: stg_payments
    columns:
      - name: payment_id
        data_tests:
          - unique
          - not_null
      - name: payment_method
        data_tests:
          - accepted_values:
              values: ['credit_card', 'coupon', 'bank_transfer', 'gift_card']
"""

# models/staging/stg_customers.sql
staging_stg_customers_sql = """
with source as (

    {#-
    Normally we would select from the table here, but we are using seeds to load
    our data in this project
    #}
    select * from {{ ref('raw_customers') }}

),

renamed as (

    select
        id as customer_id,
        first_name,
        last_name

    from source

)

select * from renamed
"""

# models/staging/stg_orders.sql
staging_stg_orders_sql = """
with source as (

    {#-
    Normally we would select from the table here, but we are using seeds to load
    our data in this project
    #}
    select * from {{ ref('raw_orders') }}

),

renamed as (

    select
        id as order_id,
        user_id as customer_id,
        order_date,
        status

    from source

)

select * from renamed
"""

# models/staging/stg_payments.sql
staging_stg_payments_sql = """
with source as (

    {#-
    Normally we would select from the table here, but we are using seeds to load
    our data in this project
    #}
    select * from {{ ref('raw_payments') }}

),

renamed as (

    select
        id as payment_id,
        order_id,
        payment_method,

        -- `amount` is currently stored in cents, so we convert it to dollars
        amount / 100 as amount

    from source

)

select * from renamed
"""


class JaffleShopProject:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "docs.md": docs_md,
            "orders.sql": orders_sql,
            "ignored_model1.sql": "select 1 as id",
            "ignored_model2.sql": "select 1 as id",
            "overview.md": overview_md,
            "schema.yml": schema_yml,
            "ignore_folder": {
                "model1.sql": "select 1 as id",
                "model2.sql": "select 1 as id",
            },
            "staging": {
                "schema.yml": staging_schema_yml,
                "stg_customers.sql": staging_stg_customers_sql,
                "stg_orders.sql": staging_stg_orders_sql,
                "stg_payments.sql": staging_stg_payments_sql,
            },
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        # Read seed file and return
        seeds = {}
        dir_path = os.path.dirname(os.path.realpath(__file__))
        for file_name in ("raw_customers.csv", "raw_orders.csv", "raw_payments.csv"):
            contents = read_file(dir_path, "jaffle_shop_data", file_name)
            seeds[file_name] = contents
        return seeds

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "name": "jaffle_shop",
            "models": {
                "jaffle_shop": {
                    "materialized": "table",
                    "staging": {
                        "materialized": "view",
                    },
                }
            },
        }
