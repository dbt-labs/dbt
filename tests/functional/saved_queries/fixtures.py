saved_query_description = """
{% docs saved_query_description %} My SavedQuery Description {% enddocs %}
"""

saved_queries_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
            - "{{ Metric('txn_revenue', ['id']) }} > 1"
        order_by:
            - Metric('simple_metric').descending(True)
            - "Dimension('id__ds').descending(True)"
        limit: 10
    exports:
        - name: my_export
          config:
            alias: my_export_alias
            export_as: table
            schema: my_export_schema_name
"""

saved_queries_with_defaults_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
            - "{{ Metric('txn_revenue', ['id']) }} > 1"
    exports:
        - name: my_export
          config:
            alias: my_export_alias
            export_as: table
"""

saved_queries_with_diff_filters_yml = """
saved_queries:
  - name: test_saved_query_where_list
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            alias: my_export_alias
            export_as: table
            schema: my_export_schema_name

  - name: test_saved_query_where_str
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query2
    query_params:
      metrics:
        - simple_metric
      group_by:
        - "Dimension('id__ds')"
      where: "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
"""

saved_query_with_extra_config_attributes_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            my_random_config: 'I have this for some reason'
            export_as: table
"""

saved_query_with_export_configs_defined_at_saved_query_level_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    config:
      export_as: table
      schema: my_default_export_schema
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            export_as: view
            schema: my_custom_export_schema
        - name: my_export2
"""

saved_query_with_export_database_configs_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    config:
      export_as: table
      database: my_default_export_database
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            database: my_custom_export_database
        - name: my_export2
"""

saved_query_without_export_configs_defined_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
"""

saved_query_with_cache_configs_defined_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    config:
      cache:
        enabled: True
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            alias: my_export_alias
            export_as: table
            schema: my_export_schema_name
"""

saved_query_with_tags_defined_yml = """
saved_queries:
  - name: test_saved_query
    description: "{{ doc('saved_query_description') }}"
    label: Test Saved Query
    tags:
        - tag_a
        - tag_c
    query_params:
        metrics:
            - simple_metric
        group_by:
            - "Dimension('id__ds')"
        where:
            - "{{ TimeDimension('id__ds', 'DAY') }} <= now()"
            - "{{ TimeDimension('id__ds', 'DAY') }} >= '2023-01-01'"
    exports:
        - name: my_export
          config:
            alias: my_export_alias
            export_as: table
            schema: my_export_schema_name
"""
