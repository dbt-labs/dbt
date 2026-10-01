{% macro bq_validate_microbatch_config(config) %}
  {% if config.get("partition_by") is none %}
    {% set missing_partition_msg -%}
    The 'microbatch' strategy requires a `partition_by` config.
    {%- endset %}
    {% do exceptions.raise_compiler_error(missing_partition_msg) %}
  {% endif %}

  {% if config.get("partition_by").granularity != config.get('batch_size') %}
    {% set invalid_partition_by_granularity_msg -%}
    The 'microbatch' strategy requires a `partition_by` config with the same granularity as its configured `batch_size`.
    Got:
      `batch_size`: {{ config.get('batch_size') }}
      `partition_by.granularity`: {{ config.get("partition_by").granularity }}
    {%- endset %}
    {% do exceptions.raise_compiler_error(invalid_partition_by_granularity_msg) %}
  {% endif %}
{% endmacro %}

{% macro bq_generate_microbatch_build_sql(
      tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
) %}
    {% set build_sql = bq_insert_overwrite_sql(
        tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
    ) %}

    {{ return(build_sql) }}
{% endmacro %}
