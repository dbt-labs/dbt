models_sql = """
select 1 as id
"""

second_model_sql = """
select 1 as id
"""


metricflow_time_spine_sql = """
SELECT to_date('02/20/2023', 'mm/dd/yyyy') as date_day
"""


source_schema_yml = """version: 2

sources:
  - name: test_source
    tables:
      - name: test_table
"""


semantic_models_schema_yml = """version: 2

semantic_models:
  - name: semantic_model
    model: ref('model')
    dimensions:
      - name: created_at
        type: time
    measures:
      - name: distinct_metrics
        agg: count_distinct
        expr: id
    entities:
      - name: model
        type: primary
        expr: id
    defaults:
      agg_time_dimension: created_at
"""


metrics_schema_yml = """version: 2

metrics:
  - name: metric
    label: "label"
    type: simple
    type_params:
      measure: "distinct_metrics"
"""

simple_exposure_yml = """
version: 2

exposures:
  - name: simple_exposure
    label: simple exposure label
    type: dashboard
    depends_on:
      - ref('model')
      - source('test_source', 'test_table')
      - metric('metric')
    owner:
      email: something@example.com
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('model')
      - ref('second_model')
    owner:
      email: something@example.com
      name: Some name
    description: >
      A description of the complex exposure
    maturity: medium
    meta:
      tool: 'my_tool'
      languages:
        - python
    tags: ['my_department']
    url: http://example.com/notebook/1
"""

disabled_models_exposure_yml = """
version: 2

exposures:
  - name: simple_exposure
    type: dashboard
    config:
      enabled: False
    depends_on:
      - ref('model')
    owner:
      email: something@example.com
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('model')
      - ref('second_model')
    owner:
      email: something@example.com
      name: Some name
    description: >
      A description of the complex exposure
    maturity: medium
    meta:
      tool: 'my_tool'
      languages:
        - python
    tags: ['my_department']
    url: http://example.com/notebook/1
"""

enabled_yaml_level_exposure_yml = """
version: 2

exposures:
  - name: simple_exposure
    type: dashboard
    config:
      enabled: True
    depends_on:
      - ref('model')
    owner:
      email: something@example.com
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('model')
      - ref('second_model')
    owner:
      email: something@example.com
      name: Some name
    description: >
      A description of the complex exposure
    maturity: medium
    meta:
      tool: 'my_tool'
      languages:
        - python
    tags: ['my_department']
    url: http://example.com/notebook/1
"""

invalid_config_exposure_yml = """
version: 2

exposures:
  - name: simple_exposure
    type: dashboard
    config:
      enabled: True and False
    depends_on:
      - ref('model')
    owner:
      email: something@example.com
"""

duplicate_exposure_yml = """
  - name: simple_exposure
    type: dashboard
    config:
      enabled: True
    depends_on:
      - ref('model')
    owner:
      email: something@example.com
"""
