{% macro bq_generate_incremental_insert_overwrite_build_sql(
    tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
) %}
    {% if partition_by is none %}
      {% set missing_partition_msg -%}
      The 'insert_overwrite' strategy requires the `partition_by` config.
      {%- endset %}
      {% do exceptions.raise_compiler_error(missing_partition_msg) %}
    {% endif %}

    {% set build_sql = bq_insert_overwrite_sql(
        tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
    ) %}

    {{ return(build_sql) }}

{% endmacro %}

{% macro bq_copy_partitions(tmp_relation, target_relation, partitions, partition_by) %}

  {% for partition in partitions %}
    {% if partition_by.data_type == 'int64' %}
      {% set partition = partition | as_text %}
    {% elif partition_by.granularity == 'hour' %}
      {% set partition = partition.strftime("%Y%m%d%H") %}
    {% elif partition_by.granularity == 'day' %}
      {% set partition = partition.strftime("%Y%m%d") %}
    {% elif partition_by.granularity == 'month' %}
      {% set partition = partition.strftime("%Y%m") %}
    {% elif partition_by.granularity == 'year' %}
      {% set partition = partition.strftime("%Y") %}
    {% endif %}
    {% set tmp_relation_partitioned = api.Relation.create(database=tmp_relation.database, schema=tmp_relation.schema, identifier=tmp_relation.table ~ '$' ~ partition, type=tmp_relation.type) %}
    {% set target_relation_partitioned = api.Relation.create(database=target_relation.database, schema=target_relation.schema, identifier=target_relation.table ~ '$' ~ partition, type=target_relation.type) %}
    {% do adapter.copy_table(tmp_relation_partitioned, target_relation_partitioned, "table") %}
  {% endfor %}

{% endmacro %}

{% macro bq_insert_overwrite_sql(
    tmp_relation, target_relation, sql, unique_key, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
) %}

    {# static #}
    {% if partitions is not none and partitions != [] %}
        {{ bq_static_insert_overwrite_sql(tmp_relation, target_relation, sql, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions) }}

    {# dynamic #}
    {% else %}
        {{ bq_dynamic_insert_overwrite_sql(tmp_relation, target_relation, sql, unique_key, partition_by, dest_columns, tmp_relation_exists, copy_partitions) }}
    {% endif %}

{% endmacro %}

{#
    -- static partitions refer to a fixed set of partition values that are
    -- known ahead of time and do not depend on source data or runtime
    -- conditions. these values are typically hardcoded or configured
    -- externally (e.g., via dbt variables or macros).
    --
    -- with `insert_overwrite`, dbt uses a predefined partition filter
    -- expression to overwrite specific partitions
    -- (e.g., where partition_column = '2024-05-01').
    -- it excels where using a fixed minimal partition list, no need to
    -- query source data for partitions, and insert statements can be
    -- batched. this also keeps things deterministic.
#}

{% macro bq_static_select_insert_overwrite_sql(tmp_relation, sql, partition_by, tmp_relation_exists) %}
  {%- set source_sql -%}
  (
    {% if tmp_relation_exists %}
      select
        {% if partition_by.time_ingestion_partitioning %}
          {{ partition_by.insertable_time_partitioning_field() }},
        {% endif %}
        * from {{ tmp_relation }}
    {%- elif partition_by.time_ingestion_partitioning -%}
      {{ wrap_with_time_ingestion_partitioning_sql(partition_by, sql, true) }}
    {%- else -%}
      {{ sql }}
    {%- endif %}
  )
  {%- endset -%}
  {{ return(source_sql) }}
{% endmacro %}


{#
  -- Static-partition + `copy_partitions=true` should inline the literals / foldable constants
  -- supplied via the `partitions=` config.

  -- The inline method will still copy (truncate) a partition even
  -- if the incremental run produced no rows for that date, because the user
  -- explicitly listed it in `partitions=`.
#}
{% macro bq_static_copy_partitions_insert_overwrite_sql(
  tmp_relation, target_relation, sql, partition_by, partitions, tmp_relation_exists
) %}

    {%- if tmp_relation_exists is false -%}
        {%- set source_sql = bq_static_select_insert_overwrite_sql(tmp_relation, sql, partition_by, tmp_relation_exists) %}

        {# -- we run temp table creation in a separate script to move to partitions copy if it doesn't already exist #}
        {%- call statement('create_tmp_relation_for_copy', language='sql') -%}
          {{ bq_create_table_as(partition_by, true, tmp_relation, source_sql, 'sql')
        }}
        {%- endcall %}
    {%- endif -%}

    {%- set partitions_sql -%}
	select
	    cast(partition_literal as timestamp) as partition_ts
	from unnest([
	    {{ partitions | join(', ') }}
	]) as partition_literal
    {%- endset -%}

    {%- set resolved_partitions = run_query(partitions_sql).columns[0].values() -%}

    {% do bq_copy_partitions(tmp_relation, target_relation, resolved_partitions, partition_by) %}

    {# clean up temp table #}
    drop table if exists {{ tmp_relation }}
{% endmacro %}


{% macro bq_static_insert_overwrite_sql(
    tmp_relation, target_relation, sql, partition_by, partitions, dest_columns, tmp_relation_exists, copy_partitions
) %}

    {%- if copy_partitions %}
        {{ bq_static_copy_partitions_insert_overwrite_sql(tmp_relation, target_relation, sql, partition_by, partitions, tmp_relation_exists) }}
    {% else -%}
        {% set predicate -%}
            {{ partition_by.render_wrapped(alias='dbt_internal_dest') }} in (
                {{ partitions | join (', ') }}
            )
        {%- endset %}

        {%- set source_sql = bq_static_select_insert_overwrite_sql(tmp_relation, sql, partition_by, tmp_relation_exists) %}
	{#
	  -- when the model sql is inserted directly into the merge statement,
	  -- we need to prepend it with the user-defined `sql_header`. this is
	  -- important when the model sql references elements like variables or udfs
	  -- defined in the header.

	  -- in the case where a temporary table is created first (i.e., the
	  -- "temp table exists" path), the `sql_header` is already included via
	  -- the `create_table_as` macro, so no additional handling is needed.
	#}

        -- 1. run the merge statement
        {{ get_insert_overwrite_merge_sql(target_relation, source_sql, dest_columns, [predicate], include_sql_header = not tmp_relation_exists) }};

        {%- if tmp_relation_exists -%}
        -- 2. clean up the temp table
        drop table if exists {{ tmp_relation }};
        {%- endif -%}

    {%- endif -%}
{% endmacro %}


{#
    -- dynamic partitions refer to the set partition values that are
    -- determined at runtime, based on either the contents of source
    -- data (e.g. values in an updated_date column) or external runtime
    -- parameters (e.g. time of day, system params).
    --
    -- with `insert_overwrite`, this allows dbt to compute which
    -- partitions to overwrite dynamically using a partition filter
    -- expression (e.g., where partition_column >= current_date()).
    --
    -- this enables targeted incremental updates by overwriting only the
    -- affected partitions, rather than replacing the entire table.
    -- this reduces latency and cost by limiting the scope of writes.
#}


{% macro bq_dynamic_copy_partitions_insert_overwrite_sql(
  tmp_relation, target_relation, sql, unique_key, partition_by, dest_columns, tmp_relation_exists, copy_partitions
  ) %}
  {%- if tmp_relation_exists is false -%}
  {# We run temp table creation in a separated script to move to partitions copy if it does not already exist #}
    {%- call statement('create_tmp_relation_for_copy', language='sql') -%}
      {{ bq_create_table_as(partition_by, True, tmp_relation, sql, 'sql')
    }}
    {%- endcall %}
  {%- endif -%}
  {%- set partitions_sql -%}
    select distinct {{ partition_by.render_wrapped() }}
    from {{ tmp_relation }}
  {%- endset -%}
  {# DIVERGENCE since otherwise the query will be executed using a client with the Storage Read API enabled #}
  {# that would return null values for the partition pseudo-columns #}
  {%- set partitions = _run_query_with_options(
    partitions_sql,
    options={
      "adbc.bigquery.sql.query.use_storage_api_disabled_client": "1"
    }
  ).columns[0].values() -%}
  {# We copy the partitions #}
  {%- do bq_copy_partitions(tmp_relation, target_relation, partitions, partition_by) -%}
  -- Clean up the temp table
  drop table if exists {{ tmp_relation }}
{% endmacro %}

{% macro bq_dynamic_insert_overwrite_sql(tmp_relation, target_relation, sql, unique_key, partition_by, dest_columns, tmp_relation_exists, copy_partitions) %}
  {%- if copy_partitions is true %}
     {{ bq_dynamic_copy_partitions_insert_overwrite_sql(tmp_relation, target_relation, sql, unique_key, partition_by, dest_columns, tmp_relation_exists, copy_partitions) }}
  {% else -%}
      {% set predicate -%}
          {{ partition_by.render_wrapped(alias='DBT_INTERNAL_DEST') }} in unnest(dbt_partitions_for_replacement)
      {%- endset %}

      {%- set source_sql -%}
      (
        select
        {% if partition_by.time_ingestion_partitioning -%}
        {{ partition_by.insertable_time_partitioning_field() }},
        {%- endif -%}
        * from {{ tmp_relation }}
      )
      {%- endset -%}

      -- generated script to merge partitions into {{ target_relation }}
      declare dbt_partitions_for_replacement array<{{ partition_by.data_type_for_partition() }}>;

      {# have we already created the temp table to check for schema changes? #}
      {% if not tmp_relation_exists %}
       -- 1. create a temp table with model data
        {{ bq_create_table_as(partition_by, True, tmp_relation, sql, 'sql') }}
      {% else %}
        -- 1. temp table already exists, we used it to check for schema changes
      {% endif %}
      {%- set partition_field = partition_by.time_partitioning_field() if partition_by.time_ingestion_partitioning else partition_by.render_wrapped() -%}

      -- 2. define partitions to update
      set (dbt_partitions_for_replacement) = (
          select as struct
              -- IGNORE NULLS: this needs to be aligned to _dbt_max_partition, which ignores null
              array_agg(distinct {{ partition_field }} IGNORE NULLS)
          from {{ tmp_relation }}
      );

      -- 3. run the merge statement
      {{ get_insert_overwrite_merge_sql(target_relation, source_sql, dest_columns, [predicate]) }};

      -- 4. clean up the temp table
      drop table if exists {{ tmp_relation }}

  {% endif %}

{% endmacro %}
