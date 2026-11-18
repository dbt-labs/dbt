# not strictly necessary, but this reflects the integration tests currently in the 'dbt-metrics' package right now
# i'm including just the first 10 rows for more concise 'git diff'

mock_purchase_data_csv = """purchased_at,payment_type,payment_total
2021-02-14 17:52:36,maestro,2418.94
2021-02-15 04:16:50,jcb,3043.28
2021-02-15 11:30:45,solo,1505.81
2021-02-16 13:08:18,,1532.85
2021-02-17 05:41:34,americanexpress,319.91
2021-02-18 06:47:32,jcb,2143.44
2021-02-19 01:37:09,jcb,840.1
2021-02-19 03:38:49,jcb,1388.18
2021-02-19 04:22:41,jcb,2834.96
2021-02-19 13:28:50,china-unionpay,2440.98
""".strip()

models_people_sql = """
select 1 as id, 'Drew' as first_name, 'Banin' as last_name, 'yellow' as favorite_color, true as loves_dbt, 5 as tenure, current_timestamp as created_at
union all
select 2 as id, 'Jeremy' as first_name, 'Cohen' as last_name, 'indigo' as favorite_color, true as loves_dbt, 4 as tenure, current_timestamp as created_at
union all
select 3 as id, 'Callum' as first_name, 'McCann' as last_name, 'emerald' as favorite_color, true as loves_dbt, 0 as tenure, current_timestamp as created_at
"""

semantic_model_people_yml = """
version: 2

semantic_models:
  - name: semantic_people
    model: ref('people')
    dimensions:
      - name: favorite_color
        type: categorical
      - name: created_at
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        agg: SUM
        expr: tenure
      - name: people
        agg: count
        expr: id
    entities:
      - name: id
        type: primary
    defaults:
      agg_time_dimension: created_at
"""

basic_metrics_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: "years_tenure"
        filter: "{{ Dimension('id__loves_dbt') }} is true"

  - name: average_tenure
    label: "Average tenure"
    description: "The average tenure per person"
    type: ratio
    type_params:
      numerator: collective_tenure
      denominator: number_of_people

  - name: average_tenure_plus_one
    label: "Average tenure, plus 1"
    description: "The average tenure per person"
    type: derived
    type_params:
      metrics:
        - average_tenure
      expr: "average_tenure + 1"

  - name: tenured_people
    label: Tenured People
    description: People who have been here more than 1 year
    type: simple
    type_params:
      measure: people
    filter: "{{ Metric('collective_tenure', ['id']) }} > 2"
"""

metricflow_time_spine_sql = """
SELECT to_date('02/20/2023, 'mm/dd/yyyy') as date_day
"""

metricflow_time_spine_yml = """
version: 2

models:
  - name: metricflow_time_spine
    time_spine:
      standard_granularity_column: date_day
    columns:
      - name: date_day
        granularity: day
"""

models_people_metrics_shared_tag_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    time_granularity: month
    config:
      tags:
        - shared_tag

"""

models_people_metrics_top_level_tags_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    time_granularity: month
    tags:
      - top_level_tag

"""

models_people_metrics_tags_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    time_granularity: month
    config:
      tags:
        - yaml_tag

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"
        join_to_timespine: true
        fill_nulls_with: 0

  - name: average_tenure
    label: Average Tenure
    description: The average tenure of our people
    type: ratio
    type_params:
      numerator: collective_tenure
      denominator: number_of_people

  - name: average_tenure_minus_people
    label: Average Tenure minus People
    description: Well this isn't really useful is it?
    type: derived
    type_params:
      expr: average_tenure - number_of_people
      metrics:
        - average_tenure
        - number_of_people

"""

models_people_metrics_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    time_granularity: month
    config:
      meta:
        my_meta_config: 'config'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"
        join_to_timespine: true
        fill_nulls_with: 0

  - name: average_tenure
    label: Average Tenure
    description: The average tenure of our people
    type: ratio
    type_params:
      numerator: collective_tenure
      denominator: number_of_people

  - name: average_tenure_minus_people
    label: Average Tenure minus People
    description: Well this isn't really useful is it?
    type: derived
    type_params:
      expr: average_tenure - number_of_people
      metrics:
        - average_tenure
        - number_of_people

"""

models_people_metrics_meta_top_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta_top: 'top'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"
        join_to_timespine: true
        fill_nulls_with: 0

  - name: average_tenure
    label: Average Tenure
    description: The average tenure of our people
    type: ratio
    type_params:
      numerator: collective_tenure
      denominator: number_of_people

  - name: average_tenure_minus_people
    label: Average Tenure minus People
    description: Well this isn't really useful is it?
    type: derived
    type_params:
      expr: average_tenure - number_of_people
      metrics:
        - average_tenure
        - number_of_people

"""

invalid_models_people_metrics_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    model: "ref(people)"
    calculation_method: count
    expression: "*"
    timestamp: created_at
    time_grains: [day, week, month]
    dimensions:
      - favorite_color
      - loves_dbt
    meta:
        my_meta: 'testing'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    model: "ref(people)"
    calculation_method: sum
    expression: tenure
    timestamp: created_at
    time_grains: [day]
    filters:
      - field: loves_dbt
        operator: 'is'
        value: 'true'

"""

invalid_metrics_missing_model_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    calculation_method: count
    expression: "*"
    timestamp: created_at
    time_grains: [day, week, month]
    dimensions:
      - favorite_color
      - loves_dbt
    meta:
        my_meta: 'testing'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    calculation_method: sum
    expression: tenure
    timestamp: created_at
    time_grains: [day]
    filters:
      - field: loves_dbt
        operator: 'is'
        value: 'true'

"""

invalid_metrics_missing_expression_yml = """
version: 2
metrics:
  - name: number_of_people
    label: "Number of people"
    model: "ref(people)"
    description: Total count of people
    calculation_method: count
    timestamp: created_at
    time_grains: [day, week, month]
    dimensions:
      - favorite_color
      - loves_dbt
    meta:
        my_meta: 'testing'
"""

names_with_spaces_metrics_yml = """
version: 2

metrics:

  - name: number of people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'

"""

names_with_special_chars_metrics_yml = """
version: 2

metrics:

  - name: number_of_people!
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'

"""


names_with_leading_numeric_metrics_yml = """
version: 2

metrics:

  - name: 1_number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'

"""

long_name_metrics_yml = """
version: 2

metrics:

  - name: this_name_is_going_to_contain_more_than_250_characters_but_be_otherwise_acceptable_and_then_will_throw_an_error_which_I_expect_to_happen_and_repeat_this_name_is_going_to_contain_more_than_250_characters_but_be_otherwise_acceptable_and_then_will_throw_an_error_which_I_expect_to_happen
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'

"""

downstream_model_sql = """
-- this model will depend on these three metrics
{% set some_metrics = [
    metric('count_orders'),
    metric('sum_order_revenue'),
    metric('average_order_value')
] %}

/*
{% if not execute %}

    -- the only properties available to us at 'parse' time are:
    --      'metric_name'
    --      'package_name' (None if same package)

    {% set metric_names = [] %}
    {% for m in some_metrics %}
        {% do metric_names.append(m.metric_name) %}
    {% endfor %}

    -- this config does nothing, but it lets us check these values below
    {{ config(metric_names = metric_names) }}

{% else %}

    -- these are the properties available to us at 'execution' time

    {% for m in some_metrics %}
        name: {{ m.name }}
        label: {{ m.label }}
        type: {{ m.type }}
        type_params: {{ m.type_params }}
        filter: {{ m.filter }}
    {% endfor %}

{% endif %}

select 1 as id
"""

invalid_derived_metric_contains_model_yml = """
version: 2
metrics:
    - name: count_orders
      label: Count orders
      model: ref('mock_purchase_data')

      calculation_method: count
      expression: "*"
      timestamp: purchased_at
      time_grains: [day, week, month, quarter, year]

      dimensions:
        - payment_type

    - name: sum_order_revenue
      label: Total order revenue
      model: ref('mock_purchase_data')

      calculation_method: sum
      expression: "payment_total"
      timestamp: purchased_at
      time_grains: [day, week, month, quarter, year]

      dimensions:
        - payment_type

    - name: average_order_value
      label: Average Order Value

      calculation_method: derived
      expression:  "{{metric('sum_order_revenue')}} / {{metric('count_orders')}} "
      model: ref('mock_purchase_data')
      timestamp: purchased_at
      time_grains: [day, week, month, quarter, year]

      dimensions:
        - payment_type
"""

purchasing_model_sql = """
select purchased_at, payment_type, payment_total from {{ ref('mock_purchase_data') }}
"""

semantic_model_purchasing_yml = """
version: 2

semantic_models:
  - name: semantic_purchasing
    model: ref('purchasing')
    measures:
      - name: num_orders
        agg: COUNT
        expr: purchased_at
      - name: order_revenue
        agg: SUM
        expr: payment_total
    dimensions:
      - name: purchased_at
        type: TIME
        type_params:
          time_granularity: day
    entities:
      - name: purchase
        type: primary
        expr: '1'
    defaults:
      agg_time_dimension: purchased_at

"""

derived_metric_yml = """
version: 2
metrics:
    - name: count_orders
      label: Count orders
      type: simple
      type_params:
        measure: num_orders

    - name: sum_order_revenue
      label: Total order revenue
      type: simple
      type_params:
        measure: order_revenue

    - name: average_order_value
      label: Average Order Value
      type: ratio
      type_params:
        numerator:
          name: sum_order_revenue
        denominator:
          name: count_orders

    - name: sum_order_revenue_plus_one_custom_offset_window
      label: "Total order revenue, plus 1 with custom offset window"
      description: "The total order revenue plus 1 offset by 1 martian day"
      type: derived
      type_params:
        metrics:
          - name: sum_order_revenue
            offset_window: 1 martian_day
        expr: "sum_order_revenue + 1"
"""

disabled_metric_level_schema_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    config:
      enabled: False
    meta:
        my_meta: 'testing'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"

"""

meta_metric_level_schema_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    config:
      meta:
         my_meta_config: 'config
    meta:
        my_meta_direct: 'direct'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"

"""

enabled_metric_level_schema_yml = """
version: 2

metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    config:
      enabled: True
    meta:
        my_meta: 'testing'

  - name: collective_tenure
    label: "Collective tenure"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: years_tenure
        filter: "{{ Dimension('id__loves_dbt') }} is true"

"""

models_people_metrics_sql = """
-- this model will depend on these two metrics
{% set some_metrics = [
    metric('number_of_people'),
    metric('collective_tenure')
] %}

/*
{% if not execute %}

    -- the only properties available to us at 'parse' time are:
    --      'metric_name'
    --      'package_name' (None if same package)

    {% set metric_names = [] %}
    {% for m in some_metrics %}
        {% do metric_names.append(m.metric_name) %}
    {% endfor %}

    -- this config does nothing, but it lets us check these values below
    {{ config(metric_names = metric_names) }}

{% else %}

    -- these are the properties available to us at 'execution' time

    {% for m in some_metrics %}
        name: {{ m.name }}
        label: {{ m.label }}
        type: {{ m.type }}
        type_params: {{ m.type_params }}
        filter: {{ m.filter }}
        window: {{ m.window }}
    {% endfor %}

{% endif %}

select 1 as id
"""

metrics_1_yml = """
version: 2

metrics:
  - name: some_metric
    label: Some Metric
    type: simple
    type_params:
      measure: some_measure
"""

metrics_2_yml = """
version: 2

metrics:
  - name: some_metric
    label: Some Metric
    type: simple
    type_params:
      measure: some_measure
"""

model_a_sql = """
select 1 as fun
"""

model_b_sql = """
-- {{ metric('some_metric') }}

{% if execute %}
  {% set model_ref_node = graph.nodes.values() | selectattr('name', 'equalto', 'model_a') | first %}
  {% set relation = api.Relation.create(
      database = model_ref_node.database,
      schema = model_ref_node.schema,
      identifier = model_ref_node.alias
  )
  %}
{% else %}
  {% set relation = "" %}
{% endif %}

-- this one is a real ref
select * from {{ ref('model_a') }}
union all
-- this one is synthesized via 'graph' var
select * from {{ relation }}
"""

invalid_config_metric_yml = """
version: 2

metrics:
  - name: number_of_people
    label: "Number of people"
    config:
        enabled: True and False
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'
"""

invalid_metric_without_timestamp_with_time_grains_yml = """
version: 2

metrics:
  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    model: "ref('people')"
    time_grains: [day, week, month]
    calculation_method: count
    expression: "*"
    dimensions:
      - favorite_color
      - loves_dbt
    meta:
        my_meta: 'testing'
"""

invalid_metric_without_timestamp_with_window_yml = """
version: 2

metrics:
  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    model: "ref('people')"
    window:
      count: 14
      period: day
    calculation_method: count
    expression: "*"
    dimensions:
      - favorite_color
      - loves_dbt
    meta:
        my_meta: 'testing'
"""

conversion_semantic_model_purchasing_yml = """
version: 2

semantic_models:
  - name: semantic_purchasing
    model: ref('purchasing')
    measures:
      - name: num_orders
        agg: COUNT
        expr: purchased_at
      - name: num_visits
        agg: SUM
        expr: 1
    dimensions:
      - name: purchased_at
        type: TIME
        type_params:
          time_granularity: day
    entities:
      - name: purchase
        type: primary
        expr: '1'
    defaults:
      agg_time_dimension: purchased_at

"""

cumulative_metric_yml = """
version: 2
metrics:
    - name: weekly_visits
      label: Rolling sum of visits over the last 7 days
      type: cumulative
      type_params:
        measure: num_visits
        cumulative_type_params:
          window: 7 days
          period_agg: average
    - name: cumulative_orders
      label: Rolling total of orders (all time)
      type: cumulative
      type_params:
        measure: num_orders
        cumulative_type_params:
          period_agg: last
    - name: orders_ytd
      label: Total orders since the start of the year
      type: cumulative
      type_params:
        measure: num_orders
        cumulative_type_params:
          grain_to_date: year
          period_agg: first
    - name: monthly_orders
      label: Orders in the past month
      type: cumulative
      type_params:
        measure: num_orders
        cumulative_type_params:
          window: 1 month
          period_agg: average
    - name: yearly_orders
      label: Orders in the past year
      type: cumulative
      type_params:
        measure: num_orders
        cumulative_type_params:
          window: 1 year
          period_agg: first
    - name: visits_mtd
      label: Visits since start of month
      type: cumulative
      type_params:
        measure: num_visits
        cumulative_type_params:
          grain_to_date: month
          period_agg: first
    - name: cumulative_visits
      label: Rolling total of visits (all time)
      type: cumulative
      type_params:
        measure: num_visits
        cumulative_type_params:
          period_agg: first
    # TODO: Re-enable this when custom grain is supported for this type
    # - name: visits_martian_day
    #   label: Visits since start of martian_day
    #   type: cumulative
    #   type_params:
    #     measure: num_visits
    #     cumulative_type_params:
    #       grain_to_date: martian_day
    # - name: visits_martian_day_window
    #   label: Visits since start of martian_day window
    #   type: cumulative
    #   type_params:
    #     measure: num_visits
    #     cumulative_type_params:
    #       window: 1 martian_day
"""

conversion_metric_yml = """
version: 2
metrics:
    - name: converted_orders_over_visits
      label: Number of orders converted from visits
      type: conversion
      type_params:
        conversion_type_params:
          base_measure: num_visits
          conversion_measure: num_orders
          entity: purchase
    - name: converted_orders_over_visits_with_window
      label: Number of orders converted from visits with window
      type: conversion
      type_params:
        conversion_type_params:
          base_measure: num_visits
          conversion_measure: num_orders
          entity: purchase
          window: 4 day
    # TODO: Re-enable this when custom grain is supported for this type
    # - name: converted_orders_over_visits_with_custom_window
    #   label: Number of orders converted from visits with custom window
    #   type: conversion
    #   type_params:
    #     conversion_type_params:
    #       base_measure: num_visits
    #       conversion_measure: num_orders
    #       entity: purchase
    #       window: 4 martian_day
"""

filtered_metrics_yml = """
version: 2

metrics:

  - name: collective_tenure_measure_filter_str
    label: "Collective tenure1"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: "years_tenure"
        filter: "{{ Dimension('id__loves_dbt') }} is true"

  - name: collective_tenure_measure_filter_list
    label: "Collective tenure2"
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: "years_tenure"
        filter:
          - "{{ Dimension('id__loves_dbt') }} is true"

  - name: collective_tenure_metric_filter_str
    label: Collective tenure3
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: "years_tenure"
    filter: "{{ Dimension('id__loves_dbt') }} is true"

  - name: collective_tenure_metric_filter_list
    label: Collective tenure4
    description: Total number of years of team experience
    type: simple
    type_params:
      measure:
        name: "years_tenure"
    filter:
      - "{{ Dimension('id__loves_dbt') }} is true"

  - name: average_tenure_filter_str
    label: Average tenure of people who love dbt1
    description: Average tenure of people who love dbt
    type: derived
    type_params:
      expr: "average_tenure"
      metrics:
        - name: average_tenure
          filter: "{{ Dimension('id__loves_dbt') }} is true"

  - name: average_tenure_filter_list
    label: Average tenure of people who love dbt2
    description: Average tenure of people who love dbt
    type: derived
    type_params:
      expr: "average_tenure"
      metrics:
        - name: average_tenure
          filter:
            - "{{ Dimension('id__loves_dbt') }} is true"
"""

duplicate_measure_metric_yml = """
metrics:
  # Simple metrics
  - name: people_with_tenure
    description: "Count of people with tenure"
    type: simple
    label: People with tenure
    type_params:
      measure: people
  - name: ratio_tenure_to_people
    description: People to years of tenure
    label: New customers to all customers
    type: ratio
    type_params:
      numerator: people_with_tenure
      denominator: number_of_people
"""
