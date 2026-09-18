{% macro validate_get_incremental_strategy(raw_strategy, table_type) %}
  {%- if table_type == 'iceberg' -%}
    {% set invalid_strategy_msg -%}
      Invalid incremental strategy provided: {{ raw_strategy }}
      Incremental models on Iceberg tables only work with 'append', 'microbatch' or 'merge' (v3 only) strategy.
    {%- endset %}
    {% if raw_strategy not in ['append', 'microbatch', 'merge'] %}
      {% do exceptions.raise_compiler_error(invalid_strategy_msg) %}
    {% endif %}
    {% if raw_strategy == 'microbatch' %}
      {% do return('merge') %}
    {% endif %}
  {%- else -%}
    {% set invalid_strategy_msg -%}
      Invalid incremental strategy provided: {{ raw_strategy }}
      Expected one of: 'append', 'insert_overwrite', 'microbatch'
    {%- endset %}

    {% if raw_strategy not in ['append', 'insert_overwrite', 'microbatch'] %}
      {% do exceptions.raise_compiler_error(invalid_strategy_msg) %}
    {% endif %}

    {% if raw_strategy == 'microbatch' %}
      {% do return('insert_overwrite') %}
    {% endif %}
  {% endif %}

  {% do return(raw_strategy) %}
{% endmacro %}


{% macro batch_incremental_insert(tmp_relation, target_relation, dest_cols_csv) %}
    {% set partitions_batches = get_partition_batches(tmp_relation) %}
    {% do log('BATCHES TO PROCESS: ' ~ partitions_batches | length) %}
    {%- for batch in partitions_batches -%}
        {%- do log('BATCH PROCESSING: ' ~ loop.index ~ ' OF ' ~ partitions_batches|length) -%}
        {%- set insert_batch_partitions -%}
            insert into {{ target_relation }} ({{ dest_cols_csv }})
                (
                   select {{ dest_cols_csv }}
                   from {{ tmp_relation }}
                   where {{ batch }}
                );
        {%- endset -%}
        {%- do run_query(insert_batch_partitions) -%}
    {%- endfor -%}
{% endmacro %}


{% macro incremental_insert(
    on_schema_change,
    tmp_relation,
    target_relation,
    existing_relation,
    force_batch,
    statement_name="main"
  )
%}
    {%- set dest_columns = process_schema_changes(on_schema_change, tmp_relation, existing_relation) -%}
    {%- if not dest_columns -%}
      {%- set dest_columns = adapter.get_columns_in_relation(target_relation) -%}
    {%- endif -%}
    {%- set dest_cols_csv = dest_columns | map(attribute='quoted') | join(', ') -%}

    {% if force_batch %}
        {% do batch_incremental_insert(tmp_relation, target_relation, dest_cols_csv) %}
    {% else %}
      {%- set insert_full -%}
          insert into {{ target_relation }} ({{ dest_cols_csv }})
              (
                 select {{ dest_cols_csv }}
                 from {{ tmp_relation }}
              );
      {%- endset -%}

      {%- set query_result =  adapter.run_query_with_partitions_limit_catching(insert_full) -%}
      {%- do log('QUERY RESULT: ' ~ query_result) -%}
      {%- if query_result == 'TOO_MANY_OPEN_PARTITIONS' -%}
          {% do batch_incremental_insert(tmp_relation, target_relation, dest_cols_csv) %}
      {%- endif -%}
    {%- endif -%}

    SELECT '{{query_result}}'

{%- endmacro %}


{% macro delete_overlapping_partitions(target_relation, tmp_relation, partitioned_by) %}
  {%- set partitioned_keys = partitioned_by | tojson | replace('\"', '') | replace('[', '') | replace(']', '') -%}
  {% call statement('get_partitions', fetch_result=True) %}
    select distinct {{partitioned_keys}} from {{ tmp_relation }};
  {% endcall %}
  {%- set table = load_result('get_partitions').table -%}
  {%- set rows = table.rows -%}
  {%- set partitions = [] -%}
  {%- for row in rows -%}
    {%- set single_partition = [] -%}
    {%- for col in row -%}
      {%- set column_type = adapter.convert_type(table, loop.index0) -%}
      {%- if column_type == 'integer' or column_type is none -%}
        {%- set value = col|string -%}
      {%- elif column_type == 'string' -%}
        {%- set value = "'" + col + "'" -%}
      {%- elif column_type == 'date' -%}
        {%- set value = "'" + col|string + "'" -%}
      {%- elif column_type == 'timestamp' -%}
        {%- set value = "'" + col|string + "'" -%}
      {%- else -%}
        {%- do exceptions.raise_compiler_error('Need to add support for column type ' + column_type) -%}
      {%- endif -%}
      {%- do single_partition.append(partitioned_by[loop.index0] + '=' + value) -%}
    {%- endfor -%}
    {%- set single_partition_expression = single_partition | join(' and ') -%}
    {%- do partitions.append(single_partition_expression) -%}
  {%- endfor -%}
  {%- do adapter.clean_up_partitions(target_relation, partitions) -%}
{%- endmacro %}

{% macro remove_partitions_from_columns(columns_with_partitions, partition_keys) %}
  {%- set columns = [] -%}
  {%- for column in columns_with_partitions -%}
    {%- if column.name not in partition_keys -%}
      {%- do columns.append(column) -%}
    {%- endif -%}
  {%- endfor -%}
  {{ return(columns) }}
{%- endmacro %}
