import textwrap

simple_metricflow_time_spine_sql = """
SELECT to_date('02/20/2023', 'mm/dd/yyyy') as date_day
"""

models_people_sql = """
select 1 as id, 'Drew' as first_name, 'Banin' as last_name, 'yellow' as favorite_color, true as loves_dbt, 5 as tenure, current_timestamp as created_at
union all
select 2 as id, 'Jeremy' as first_name, 'Cohen' as last_name, 'indigo' as favorite_color, true as loves_dbt, 4 as tenure, current_timestamp as created_at
union all
select 3 as id, 'Callum' as first_name, 'McCann' as last_name, 'emerald' as favorite_color, true as loves_dbt, 0 as tenure, current_timestamp as created_at
"""

groups_yml = """
version: 2

groups:
  - name: some_group
    owner:
      email: me@gmail.com
  - name: some_other_group
    owner:
      email: me@gmail.com
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
    meta:
        my_meta: 'testing'
"""

disabled_models_people_metrics_yml = """
version: 2

metrics:
  - name: number_of_people
    config:
      enabled: false
      group: some_group
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: people
    meta:
        my_meta: 'testing'
"""

semantic_model_people_yml = """
version: 2

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    dimensions:
      - name: favorite_color
        label: "Favorite Color"
        type: categorical
      - name: created_at
        label: "Created At"
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        label: "Years Tenure"
        agg: SUM
        expr: tenure
      - name: people
        label: "People"
        agg: count
        expr: id
    entities:
      - name: id
        label: "Primary ID"
        type: primary
    defaults:
      agg_time_dimension: created_at
"""

semantic_model_people_diff_name_yml = """
version: 2

semantic_models:
  - name: semantic_people_diff_name
    label: "Semantic People"
    model: ref('people')
    dimensions:
      - name: favorite_color
        label: "Favorite Color"
        type: categorical
      - name: created_at
        label: "Created At"
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        label: "Years Tenure"
        agg: SUM
        expr: tenure
      - name: people
        label: "People"
        agg: count
        expr: id
    entities:
      - name: id
        label: "Primary ID"
        type: primary
    defaults:
      agg_time_dimension: created_at
"""

semantic_model_descriptions = """
{% docs semantic_model_description %} foo {% enddocs %}
{% docs dimension_description %} bar {% enddocs %}
{% docs measure_description %} baz {% enddocs %}
{% docs entity_description %} qux {% enddocs %}
{% docs simple_metric_description %} describe away! {% enddocs %}
"""

semantic_model_people_yml_with_docs = """
version: 2

semantic_models:
  - name: semantic_people
    model: ref('people')
    description: "{{ doc('semantic_model_description') }}"
    dimensions:
      - name: favorite_color
        type: categorical
        description: "{{ doc('dimension_description') }}"
      - name: created_at
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        agg: SUM
        expr: tenure
        description: "{{ doc('measure_description') }}"
      - name: people
        agg: count
        expr: id
    entities:
      - name: id
        description: "{{ doc('entity_description') }}"
        type: primary
    defaults:
      agg_time_dimension: created_at
"""

enabled_semantic_model_people_yml = """
version: 2

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    config:
      enabled: true
      group: some_group
      meta:
        my_meta: 'testing'
        my_other_meta: 'testing more'
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

disabled_semantic_model_people_yml = """
version: 2

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    config:
      enabled: false
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


semantic_model_with_disabled_ref_yml = """
version: 2

models:
  - name: people
    config:
      enabled: false

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    dimensions:
      - name: favorite_color
        type: categorical
      - name: created_at
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: people
        label: "People"
        agg: count
        expr: id
    entities:
      - name: id
        type: primary
    defaults:
      agg_time_dimension: created_at
"""


base_schema_yml = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks

semantic_models:
  - name: revenue
    description: This is the revenue semantic model. It should be able to use doc blocks
    model: ref('fct_revenue')

    defaults:
      agg_time_dimension: ds

    measures:
      - name: txn_revenue
        expr: revenue
        agg: sum
        agg_time_dimension: ds
        create_metric: true
      - name: txn_revenue_with_label
        label: "Transaction Revenue with label"
        expr: revenue
        agg: sum
        agg_time_dimension: ds
        create_metric: true
      - name: sum_of_things
        expr: 2
        agg: sum
        agg_time_dimension: ds
      - name: count_of_things
        agg: count
        expr: 1
        agg_time_dimension: ds
      - name: count_of_things_2
        agg: count
        expr: 1
        agg_time_dimension: ds
      - name: has_revenue
        expr: true
        agg: sum_boolean
        agg_time_dimension: ds
      - name: discrete_order_value_p99
        expr: order_total
        agg: percentile
        agg_time_dimension: ds
        agg_params:
          percentile: 0.99
          use_discrete_percentile: True
          use_approximate_percentile: False
      - name: test_agg_params_optional_are_empty
        expr: order_total
        agg: percentile
        agg_time_dimension: ds
        agg_params:
          percentile: 0.99
      - name: test_non_additive
        expr: txn_revenue
        agg: sum
        non_additive_dimension:
          name: ds
          window_choice: max

    dimensions:
      - name: ds
        type: time
        expr: created_at
        type_params:
          time_granularity: day

    entities:
      - name: user
        type: foreign
        expr: user_id
      - name: id
        type: primary

metrics:
  - name: simple_metric
    label: Simple Metric
    type: simple
    type_params:
      measure: sum_of_things
  - name: test_cumulative_metric
    label: Cumulative Metric
    type: cumulative
    type_params:
      measure: sum_of_things
      cumulative_type_params:
        grain_to_date: day
        period_agg: first
"""

conversion_metric_yml = """
  - name: test_conversion_metric
    label: Conversion Metric
    type: conversion
    type_params:
      conversion_type_params:
        base_measure: count_of_things
        conversion_measure: count_of_things_2
        entity: user
        calculation: conversion_rate
"""

ratio_metric_yml = """
  - name: test_ratio_metric
    label: Ratio Metric
    type: ratio
    type_params:
      numerator: simple_metric
      denominator: test_conversion_metric
"""

derived_metric_yml = """
  - name: test_derived_metric
    label: Derived Metric
    type: derived
    type_params:
      metrics:
        - simple_metric
        - test_conversion_metric
      expr: simple_metric + 1
"""

schema_yml = base_schema_yml + conversion_metric_yml + ratio_metric_yml + derived_metric_yml

schema_without_semantic_model_yml = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
"""

fct_revenue_sql = """select
  1 as id,
  10 as user_id,
  1000 as revenue,
  current_timestamp as created_at"""

metricflow_time_spine_sql = """
with days as (
    {{dbt_utils.date_spine('day'
    , "to_date('01/01/2000','mm/dd/yyyy')"
    , "to_date('01/01/2027','mm/dd/yyyy')"
    )
    }}
),

final as (
    select cast(date_day as date) as date_day
    from days
)

select *
from final
"""

multi_sm_schema_yml = """
models:
  - name: fct_revenue
    description: This is the model fct_revenue.

semantic_models:
  - name: revenue
    description: This is the first semantic model.
    model: ref('fct_revenue')

    defaults:
      agg_time_dimension: ds

    measures:
      - name: txn_revenue
        expr: revenue
        agg: sum
        agg_time_dimension: ds
        create_metric: true
      - name: sum_of_things
        expr: 2
        agg: sum
        agg_time_dimension: ds

    dimensions:
      - name: ds
        type: time
        expr: created_at
        type_params:
          time_granularity: day

    entities:
      - name: user
        type: foreign
        expr: user_id
      - name: id
        type: primary

  - name: alt_revenue
    description: This is the second revenue semantic model.
    model: ref('fct_revenue')

    defaults:
      agg_time_dimension: ads

    measures:
      - name: alt_txn_revenue
        expr: revenue
        agg: sum
        agg_time_dimension: ads
        create_metric: true
      - name: alt_sum_of_things
        expr: 2
        agg: sum
        agg_time_dimension: ads

    dimensions:
      - name: ads
        type: time
        expr: created_at
        type_params:
          time_granularity: day

    entities:
      - name: user
        type: foreign
        expr: user_id
      - name: id
        type: primary

metrics:
  - name: simple_metric
    label: Simple Metric
    type: simple
    type_params:
      measure: sum_of_things
"""

semantic_model_dimensions_entities_measures_meta_config = """
version: 2

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    dimensions:
      - name: favorite_color
        label: "Favorite Color"
        type: categorical
        config:
          meta:
            dimension: one
      - name: created_at
        label: "Created At"
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        label: "Years Tenure"
        agg: SUM
        expr: tenure
        config:
          meta:
            measure: two
      - name: people
        label: "People"
        agg: count
        expr: id
    entities:
      - name: id
        label: "Primary ID"
        type: primary
        config:
          meta:
            entity: three
    defaults:
      agg_time_dimension: created_at
"""

semantic_model_meta_clobbering_yml = """
version: 2

semantic_models:
  - name: semantic_people
    label: "Semantic People"
    model: ref('people')
    config:
      meta:
        model_level: "should_be_inherited"
        component_level: "should_be_overridden"
    dimensions:
      - name: favorite_color
        label: "Favorite Color"
        type: categorical
        config:
          meta:
            component_level: "dimension_override"
      - name: created_at
        label: "Created At"
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        label: "Years Tenure"
        agg: SUM
        expr: tenure
        config:
          meta:
            component_level: "measure_override"
      - name: people
        label: "People"
        agg: count
        expr: id
    entities:
      - name: id
        label: "Primary ID"
        type: primary
        config:
          meta:
            component_level: "entity_override"
    defaults:
      agg_time_dimension: created_at
"""


semantic_model_schema_yml_v2_template_for_model_configs = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
    {semantic_model_value}
    columns:
      - name: id
        description: This is the id column dim.
        config:
          meta:
          component_level: "original_meta"
        dimension:
          name: id_dim
          label: "ID Dimension"
          type: categorical
          is_partition: true
          config:
            meta:
              component_level: "dimension_override"
        entity:
          name: id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: primary
          config:
            meta:
              component_level: "entity_override"
      - name: second_col
        description: This is the second column.
        granularity: day
        dimension:
          name: second_dim
          description: This is the second column (dim).
          label: Second Dimension
          type: time
          validity_params:
            is_start: true
            is_end: true
      - name: foreign_id_col
        description: This is a foreign id column.
        entity: foreign
      - name: col_with_default_dimensions
        description: This is the column with default dimension settings.
        dimension: categorical
        entity:
          name: col_with_default_entity_testing_default_desc
          type: natural
"""

# You can replace the semantic_model variable in the template like this:
semantic_model_schema_yml_v2 = semantic_model_schema_yml_v2_template_for_model_configs.format(
    semantic_model_value="semantic_model: true",
)

semantic_model_schema_yml_v2_disabled = (
    semantic_model_schema_yml_v2_template_for_model_configs.format(
        semantic_model_value="semantic_model: false",
    )
)

semantic_model_test_groups_yml = """groups:
  - name: finance
    owner:
      # 'name' or 'email' is required; additional properties will no longer be allowed in a future release
      email: finance@jaffleshop.com
    config:
      meta: # optional
        data_owner: Finance team
"""

semantic_model_schema_yml_v2_renamed = semantic_model_schema_yml_v2_template_for_model_configs.format(
    semantic_model_value="""semantic_model:
      name: renamed_semantic_model
      enabled: true
      group: finance
      config:
        meta:
          meta_tag_1: this_meta
    """,
)

semantic_model_schema_yml_v2_default_values = semantic_model_schema_yml_v2_template_for_model_configs.format(
    semantic_model_value="""semantic_model:
      enabled: true
    """
)

semantic_model_schema_yml_v2_disabled = semantic_model_schema_yml_v2_template_for_model_configs.format(
    semantic_model_value="""semantic_model:
      enabled: false
    """,
)

semantic_model_schema_yml_v2_false_config = (
    semantic_model_schema_yml_v2_template_for_model_configs.format(
        semantic_model_value="semantic_model: false",
    )
)

semantic_model_config_does_not_exist = (
    semantic_model_schema_yml_v2_template_for_model_configs.format(
        semantic_model_value="",
    )
)

semantic_model_schema_yml_v2_template_for_primary_entity_tests = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
    semantic_model: true
    {primary_entity_setting}
    columns:
      - name: id
        description: This is the id column dim.
        config:
          meta:
          component_level: "original_meta"
        dimension:
          type: categorical
        entity:
          name: id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: {id_entity_type}
      - name: second_col
        description: This is the second column.
        granularity: day
        dimension:
          name: second_dim
          description: This is the second column (dim).
          label: Second Dimension
          type: time
          validity_params:
            is_start: true
            is_end: true
      - name: other_id_col
        description: This is the other id column.
        entity:
          name: other_id_entity
          type: {other_id_entity_type}
      - name: col_with_default_dimensions
        description: This is the column with default dimension settings.
        dimension: categorical
        entity:
          name: col_with_default_entity_testing_default_desc
          type: natural
"""

semantic_model_schema_yml_v2_with_primary_entity_only_on_column = (
    semantic_model_schema_yml_v2_template_for_primary_entity_tests.format(
        primary_entity_setting="",
        id_entity_type="primary",
        other_id_entity_type="foreign",
    )
)

semantic_model_schema_yml_v2_primary_entity_only_on_model = (
    semantic_model_schema_yml_v2_template_for_primary_entity_tests.format(
        primary_entity_setting="primary_entity: id_entity",
        id_entity_type="foreign",
        other_id_entity_type="foreign",
    )
)

semantic_model_schema_yml_v2 = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
    semantic_model: true
    columns:
      - name: id
        description: This is the id column dim.
        config:
          meta:
          component_level: "original_meta"
        dimension:
          name: id_dim
          label: "ID Dimension"
          type: categorical
          is_partition: true
          config:
            meta:
              component_level: "dimension_override"
        entity:
          name: id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: primary
          config:
            meta:
              component_level: "entity_override"
      - name: second_col
        description: This is the second column.
        granularity: day
        dimension:
          name: second_dim
          description: This is the second column (dim).
          label: Second Dimension
          type: time
          validity_params:
            is_start: true
            is_end: true
      - name: foreign_id_col
        description: This is a foreign id column.
        entity: foreign
      - name: col_with_default_dimensions
        description: This is the column with default dimension settings.
        dimension: categorical
        entity:
          name: col_with_default_entity_testing_default_desc
          type: natural
"""

# Separate from the full-spectrum entities and dimensions test because some settings
# interact with metric validations.
base_schema_yml_v2 = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
    semantic_model: true
    agg_time_dimension: second_dim
    columns:
      - name: id
        description: This is the id column dim.
        config:
          meta:
          component_level: "original_meta"
        dimension:
          name: id_dim
          label: "ID Dimension"
          type: categorical
          is_partition: true
          config:
            meta:
              component_level: "dimension_override"
        entity:
          name: id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: primary
          config:
            meta:
              component_level: "entity_override"
      - name: second_col
        description: This is the second column.
        granularity: day
        dimension:
          name: second_dim
          description: This is the second column (dim).
          label: Second Dimension
          type: time
      - name: foreign_id_col
        description: This is a foreign id column.
        entity: foreign
      - name: created_at
        description: This is the time the entry was created.
        granularity: day
        dimension:
          name: ds
          description: the ds column
          label: DS Column
          type: time
"""

schema_yml_v2_simple_metric_on_model_1 = """
    metrics:
      - name: simple_metric
        description: This is our first simple metric.
        label: Simple Metric
        type: simple
        agg: count
        expr: id
      - name: simple_metric_2
        description: This is our second simple metric.
        agg_time_dimension: ds
        label: Simple Metric 2
        type: simple
        agg: count
        expr: second_col
      - name: percentile_metric
        description: This is our percentile metric.
        label: Percentile Metric
        type: simple
        agg: percentile
        percentile: 0.99
        percentile_type: discrete
        expr: second_col
      - name: cumulative_metric
        description: This is our cumulative metric.
        label: Cumulative Metric
        type: cumulative
        grain_to_date: day
        period_agg: first
        input_metric: simple_metric
      - name: conversion_metric
        description: This is our conversion metric.
        label: Conversion Metric
        type: conversion
        entity: id_entity
        calculation: conversion_rate
        base_metric: simple_metric
        conversion_metric: simple_metric_2
"""

schema_yml_v2_metrics_with_hidden = """
    metrics:
      - name: public_metric
        description: A metric that is not hidden.
        label: Public Metric
        type: simple
        agg: count
        expr: id
        hidden: false
      - name: private_metric
        description: A metric that is hidden.
        label: Private Metric
        type: simple
        agg: count
        expr: id
        hidden: true
"""

schema_yml_v2_metric_with_doc_jinja = """
      - name: simple_metric_with_doc_jinja
        description: "{{ doc('simple_metric_description') }}"
        label: Simple Metric With Doc Jinja
        type: simple
        agg: count
        expr: id
"""

schema_yml_v2_metric_with_filter_dimension_jinja = """
      - name: simple_metric_with_filter_dimension_jinja
        description: This is a description
        label: Simple Metric With Doc Jinja
        type: simple
        agg: count
        expr: id
        filter: |
          {{ Dimension('id_entity__id_dim') }} > 0 and {{ TimeDimension('id_entity__id_dim', 'day') }} > '2020-01-01'
"""

schema_yml_v2_metric_with_input_metrics_filter_dimension_jinja = """
    metrics:
      - name: simple_metric
        description: This is our first simple metric.
        label: Simple Metric
        type: simple
        agg: count
        expr: id
      - name: derived_metric_with_jinja_filter
        description: This is a derived metric with a jinja filter on an input metric.
        label: Derived Metric With Jinja Filter
        type: derived
        expr: simple_metric - offset_metric
        input_metrics:
          - name: simple_metric
          - name: simple_metric
            alias: offset_metric
            filter: |
              {{ Dimension('id_entity__id_dim') }} > 0
            offset_window: 1 week
"""

schema_yml_v2_metric_with_numerator_filter_dimension_jinja = """
    metrics:
      - name: simple_metric
        description: This is our first simple metric.
        label: Simple Metric
        type: simple
        agg: count
        expr: id
      - name: simple_metric_2
        description: This is our second simple metric.
        label: Simple Metric 2
        type: simple
        agg: count
        expr: second_col
      - name: ratio_metric_with_jinja_filter
        description: This is a ratio metric with a jinja filter on the numerator.
        label: Ratio Metric With Jinja Filter
        type: ratio
        numerator:
          name: simple_metric
          filter: |
            {{ Dimension('id_entity__id_dim') }} > 0
        denominator: simple_metric_2
"""

schema_yml_v2_cumulative_metric_missing_input_metric = """
    metrics:
      - name: cumulative_metric
        description: This is our cumulative metric.
        label: Cumulative Metric
        type: cumulative
        grain_to_date: day
        period_agg: first
"""

schema_yml_v2_conversion_metric_missing_base_metric = """
    metrics:
      - name: simple_metric_2
        description: This is our second simple metric.
        label: Simple Metric 2
        type: simple
        agg: count
        expr: second_col
      - name: conversion_metric
        description: This is our conversion metric.
        label: Conversion Metric
        type: conversion
        entity: id_entity
        calculation: conversion_rate
        conversion_metric: simple_metric_2
"""

schema_yml_v2_standalone_simple_metric = textwrap.dedent(schema_yml_v2_simple_metric_on_model_1)

schema_yml_v2_standalone_metrics_template = """
metrics:
  - name: standalone_conversion_metric
    description: {description}
    label: Standalone Conversion Metric
    type: conversion
    entity: id_entity
    calculation: conversion_rate
    base_metric: simple_metric
    conversion_metric: simple_metric_2
    {filter}
"""

schema_yml_v2_standalone_metrics = schema_yml_v2_standalone_metrics_template.format(
    description="This is our standalone conversion metric.",
    filter="filter: id > 0",
)

schema_yml_v2_standalone_metrics_with_doc_jinja = schema_yml_v2_standalone_metrics_template.format(
    description="\"{{ doc('simple_metric_description') }}\"",
    filter="""filter: |
      {{ Dimension('id_entity__id_dim') }} > 0""",
)

base_schema_yml_v2_with_custom_sm_name = """models:
  - name: fct_revenue
    description: This is the model fct_revenue. It should be able to use doc blocks
    semantic_model:
      name: custom_semantic_model
    agg_time_dimension: second_dim
    columns:
      - name: id
        description: This is the id column dim.
        config:
          meta:
          component_level: "original_meta"
        dimension:
          name: id_dim
          label: "ID Dimension"
          type: categorical
          is_partition: true
          config:
            meta:
              component_level: "dimension_override"
        entity:
          name: id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: primary
          config:
            meta:
              component_level: "entity_override"
      - name: second_col
        description: This is the second column.
        granularity: day
        dimension:
          name: second_dim
          description: This is the second column (dim).
          label: Second Dimension
          type: time
      - name: foreign_id_col
        description: This is a foreign id column.
        entity: foreign
      - name: created_at
        description: This is the time the entry was created.
        granularity: day
        dimension:
          name: ds
          description: the ds column
          label: DS Column
          type: time
"""

derived_semantics_yml = """
    derived_semantics:
      entities:
        - name: derived_id_entity
          description: This is the id entity, and it is the primary entity.
          label: ID Entity
          type: foreign
          expr: "id + foreign_id_col"
          config:
            meta:
              test_label_thing: derived_entity_1
        - name: derived_id_entity_with_no_optional_fields
          type: foreign
          expr: id + foreign_id_col
      dimensions:
        - name: derived_id_dimension
          type: categorical
          expr: id
          granularity: day
          validity_params:
            is_start: true
            is_end: true
"""

derived_semantics_with_doc_jinja_yml = """
    derived_semantics:
      entities:
        - name: derived_id_entity
          description: "{{ doc('entity_description') }}"
          type: foreign
          expr: "id + foreign_id_col"
      dimensions:
        - name: derived_id_dimension
          description: "{{ doc('dimension_description') }}"
          type: categorical
          expr: id
"""

semantic_model_schema_yml_v2_entity_without_name = """models:
  - name: fct_revenue
    description: This is the model fct_revenue.
    semantic_model: true
    columns:
      - name: id
        entity:
          type: primary
      - name: second_col
        granularity: day
        dimension:
          type: time
      - name: foreign_key
        entity:
          type: foreign
"""

# --- Regression fixtures for dbt-core#15294 -------------------------------
# An inline (v2) semantic model declared on a *versioned* model. The columns
# carrying the entity/dimension annotations live under versions[].columns.
versioned_model_v1_sql = """select 1 as id, cast('2020-01-01' as date) as created_at"""

versioned_model_v2_sql = """select 1 as id, cast('2020-01-01' as date) as created_at"""

versioned_model_semantic_schema_yml_v2 = """models:
  - name: metricflow_time_spine
    time_spine:
      standard_granularity_column: date_day
    columns:
      - name: date_day
        granularity: day

  - name: customers
    description: A versioned model carrying an inline semantic model.
    latest_version: 2
    semantic_model:
      enabled: true
    agg_time_dimension: created_at
    metrics:
      - name: customers_count
        description: Number of customers.
        label: Customers Count
        type: simple
        agg: count
        expr: id
    versions:
      - v: 1
        columns:
          - name: id
            entity:
              type: primary
              name: customer
          - name: created_at
            granularity: day
            dimension:
              type: time
      - v: 2
        columns:
          - name: id
            entity:
              type: primary
              name: customer
          - name: created_at
            granularity: day
            dimension:
              type: time
"""
