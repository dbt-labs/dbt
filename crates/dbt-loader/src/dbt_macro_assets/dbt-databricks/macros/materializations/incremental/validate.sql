{% macro dbt_databricks_validate_get_file_format(raw_file_format) %}
  {#-- Validate the file format #}

  {% set accepted_formats = ['text', 'csv', 'json', 'jdbc', 'parquet', 'orc', 'hive', 'delta', 'libsvm', 'hudi'] %}

  {% set invalid_file_format_msg -%}
    Invalid file format provided: {{ raw_file_format }}
    Expected one of: {{ accepted_formats | join(', ') }}
  {%- endset %}

  {% if raw_file_format not in accepted_formats %}
    {% do exceptions.raise_compiler_error(invalid_file_format_msg) %}
  {% endif %}

  {% do return(raw_file_format) %}
{% endmacro %}


{% macro dbt_databricks_validate_get_incremental_strategy(raw_strategy, file_format) %}
  {#-- Validate the incremental strategy #}

  {% set invalid_delta_only_msg -%}
    Invalid incremental strategy provided: {{ raw_strategy }}
    You can only choose this strategy when file_format is set to 'delta'
  {%- endset %}

  {% set invalid_insert_overwrite_endpoint_msg -%}
    Invalid incremental strategy provided: {{ raw_strategy }}
    You cannot use this strategy when connecting via warehouse
    Use the 'merge' or 'replace_where' strategy instead
  {%- endset %}

  {% set missing_unique_key_msg -%}
    Invalid incremental strategy provided: {{ raw_strategy }}
    This strategy requires 'unique_key' to be configured
  {%- endset %}

  {% if raw_strategy not in adapter.valid_incremental_strategies() %}
    {{ log("WARNING - You are using an unsupported incremental strategy: " ~ raw_strategy) }}
    {{ log("You can ignore this warning if you are using a custom incremental strategy") }}
  {%-else %}
    {% if raw_strategy == 'merge' and file_format not in ['delta', 'hudi'] %}
      {% do exceptions.raise_compiler_error(invalid_delta_only_msg) %}
    {% endif %}
    {% if raw_strategy in ('replace_where', 'microbatch', 'delete+insert') and file_format not in ['delta'] %}
      {% do exceptions.raise_compiler_error(invalid_delta_only_msg) %}
    {% endif %}
    {% if raw_strategy == 'delete+insert' and config.get('unique_key') is none %}
      {% do exceptions.raise_compiler_error(missing_unique_key_msg) %}
    {% endif %}
  {% endif %}

  {% do return(raw_strategy) %}
{% endmacro %}
