{% macro create_table_as(temporary, relation, compiled_code, language='sql', skip_partitioning=false) -%}
  {{ adapter.dispatch('create_table_as', 'athena')(temporary, relation, compiled_code, language, skip_partitioning) }}
{%- endmacro %}


{% macro athena__create_table_as(temporary, relation, compiled_code, language='sql', skip_partitioning=false) -%}
  {%- set materialized = config.get('materialized', default='table') -%}
  {%- set external_location = config.get('external_location', default=none) -%}
  {%- do log("Skip partitioning: " ~ skip_partitioning) -%}
  {%- set partitioned_by = config.get('partitioned_by', default=none) if not skip_partitioning else none -%}
  {%- set bucketed_by = config.get('bucketed_by', default=none) -%}
  {%- set bucket_count = config.get('bucket_count', default=none) -%}
  {%- set field_delimiter = config.get('field_delimiter', default=none) -%}
  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}
  {%- set table_type = resolve_table_type() -%}
  {# format: model `format` config > catalog file_format > parquet #}
  {%- set catalog_file_format = catalog_relation.file_format if catalog_relation is not none else none -%}
  {%- set format = config.get('format', default=none) or catalog_file_format or 'parquet' -%}
  {%- set write_compression = config.get('write_compression', default=none) -%}
  {# location base: model `s3_data_dir` config > catalog external_volume > target default #}
  {%- set catalog_external_volume = catalog_relation.external_volume if catalog_relation is not none else none -%}
  {%- set s3_data_dir = config.get('s3_data_dir', default=none) or catalog_external_volume or target.s3_data_dir -%}
  {%- set s3_data_naming = config.get('s3_data_naming', default=target.s3_data_naming) -%}
  {%- set s3_tmp_table_dir = config.get('s3_tmp_table_dir', default=target.s3_tmp_table_dir) -%}
  {%- set extra_table_properties = config.get('table_properties', default=none) -%}

  {%- set location_property = 'external_location' -%}
  {%- set partition_property = 'partitioned_by' -%}
  {%- set work_group_output_location_enforced = adapter.is_work_group_output_location_enforced() -%}
  {%- set location = adapter.generate_s3_location(relation,
                                                 s3_data_dir,
                                                 s3_data_naming,
                                                 s3_tmp_table_dir,
                                                 external_location,
                                                 temporary,
                                                 ) -%}
  {%- set native_drop = config.get('native_drop', default=false) -%}

  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(compiled_code) }}
  {%- endif -%}

  {%- if native_drop and table_type == 'iceberg' -%}
    {% do log('Config native_drop enabled, skipping direct S3 delete') %}
  {%- else -%}
    {% do adapter.delete_from_s3(location) %}
  {%- endif -%}

  {%- if language == 'python' -%}
    {%- set spark_ctas = '' -%}
    {%- if table_type == 'iceberg' -%}
      {%- set spark_ctas -%}
          create table {{ relation.schema | replace('\"', '`') }}.{{ relation.identifier | replace('\"', '`') }}
          using iceberg
          location '{{ location }}/'

          {%- if partitioned_by is not none %}
          partitioned by (
              {%- for prop_value in partitioned_by -%}
              {{ prop_value }}
              {%- if not loop.last %},{% endif -%}
              {%- endfor -%}
          )
          {%- endif %}

          {%- if extra_table_properties is not none %}
          tblproperties(
              {%- for prop_name, prop_value in extra_table_properties.items() -%}
              '{{ prop_name }}'='{{ prop_value }}'
              {%- if not loop.last %},{% endif -%}
              {%- endfor -%}
          )
          {% endif %}

          as
      {%- endset -%}
    {%- endif -%}

    {# {% do log('Creating table with spark and compiled code: ' ~ compiled_code) %} #}
    {{ athena__py_save_table_as(
        compiled_code,
        relation,
        optional_args={
          'location': location,
          'format': format,
          'mode': 'overwrite',
          'partitioned_by': partitioned_by,
          'bucketed_by': bucketed_by,
          'write_compression': write_compression,
          'bucket_count': bucket_count,
          'field_delimiter': field_delimiter,
          'spark_ctas': spark_ctas
        }
      )
    }}
  {%- else -%}
    {%- if table_type == 'iceberg' -%}
      {%- set location_property = 'location' -%}
      {%- set partition_property = 'partitioning' -%}
      {%- if bucketed_by is not none or bucket_count is not none -%}
        {%- set ignored_bucket_iceberg -%}
        bucketed_by or bucket_count cannot be used with Iceberg tables. You have to use the bucket function
        when partitioning. Will be ignored
        {%- endset -%}
        {%- set bucketed_by = none -%}
        {%- set bucket_count = none -%}
        {% do log(ignored_bucket_iceberg) %}
      {%- endif -%}
      {%- if materialized == 'table' and ( 'unique' not in s3_data_naming or external_location is not none) -%}
        {%- set error_unique_location_iceberg -%}
          You need to have an unique table location when creating Iceberg table since we use the RENAME feature
          to have near-zero downtime.
        {%- endset -%}
        {% do exceptions.raise_compiler_error(error_unique_location_iceberg) %}
      {%- endif -%}
    {%- endif %}

    create table {{ relation }}
    with (
      table_type='{{ table_type }}',
      is_external={%- if table_type == 'iceberg' -%}false{%- else -%}true{%- endif %},
    {%- if not work_group_output_location_enforced or table_type == 'iceberg' -%}
      {{ location_property }}='{{ location }}',
    {%- endif %}
    {%- if partitioned_by is not none %}
      {{ partition_property }}=ARRAY{{ partitioned_by | tojson | replace('\"', '\'') }},
    {%- endif %}
    {%- if bucketed_by is not none %}
      bucketed_by=ARRAY{{ bucketed_by | tojson | replace('\"', '\'') }},
    {%- endif %}
    {%- if bucket_count is not none %}
      bucket_count={{ bucket_count }},
    {%- endif %}
    {%- if field_delimiter is not none %}
      field_delimiter='{{ field_delimiter }}',
    {%- endif %}
    {%- if write_compression is not none %}
      write_compression='{{ write_compression }}',
    {%- endif %}
      format='{{ format }}'
    {%- if extra_table_properties is not none -%}
      {%- for prop_name, prop_value in extra_table_properties.items() -%}
      ,
      {{ prop_name }}={{ prop_value }}
      {%- endfor -%}
    {% endif %}
    )
    as
      {{ compiled_code }}
  {%- endif -%}
{%- endmacro -%}

{% macro create_table_as_with_partitions(temporary, relation, compiled_code, language='sql') -%}

    {%- set tmp_relation = api.Relation.create(
            identifier=relation.identifier ~ '__tmp_not_partitioned',
            schema=relation.schema,
            database=relation.database,
            s3_path_table_part=relation.identifier ~ '__tmp_not_partitioned' ,
            type='table'
        )
    -%}

    {%- if tmp_relation is not none -%}
      {%- do drop_relation(tmp_relation) -%}
    {%- endif -%}

    {%- do log('CREATE NON-PARTITIONED STAGING TABLE: ' ~ tmp_relation) -%}
    {%- do run_query(create_table_as(temporary, tmp_relation, compiled_code, language, true)) -%}

    {% set partitions_batches = get_partition_batches(sql=tmp_relation, as_subquery=False) %}
    {% do log('BATCHES TO PROCESS: ' ~ partitions_batches | length) %}

    {%- set dest_columns = adapter.get_columns_in_relation(tmp_relation) -%}
    {%- set dest_cols_csv = dest_columns | map(attribute='quoted') | join(', ') -%}

    {%- for batch in partitions_batches -%}
        {%- do log('BATCH PROCESSING: ' ~ loop.index ~ ' OF ' ~ partitions_batches | length) -%}

        {%- if loop.index == 1 -%}
            {%- set create_target_relation_sql -%}
                select {{ dest_cols_csv }}
                from {{ tmp_relation }}
                where {{ batch }}
            {%- endset -%}
            {%- do run_query(create_table_as(temporary, relation, create_target_relation_sql, language)) -%}
        {%- else -%}
            {%- set insert_batch_partitions_sql -%}
                insert into {{ relation }} ({{ dest_cols_csv }})
                select {{ dest_cols_csv }}
                from {{ tmp_relation }}
                where {{ batch }}
            {%- endset -%}

            {%- do run_query(insert_batch_partitions_sql) -%}
        {%- endif -%}


    {%- endfor -%}

    {%- do drop_relation(tmp_relation) -%}

    select 'SUCCESSFULLY CREATED TABLE {{ relation }}'

{%- endmacro %}

{% macro safe_create_table_as(temporary, relation, compiled_code, language='sql', force_batch=False) -%}
    {%- if language != 'sql' -%}
        {{ return(create_table_as(temporary, relation, compiled_code, language)) }}
    {%- elif force_batch -%}
      {%- do create_table_as_with_partitions(temporary, relation, compiled_code, language) -%}
      {%- set query_result = relation ~ ' with many partitions created' -%}
    {%- else -%}
        {%- if temporary -%}
          {%- do run_query(create_table_as(temporary, relation, compiled_code, language, true)) -%}
          {%- set compiled_code_result = relation ~ ' as temporary relation without partitioning created' -%}
        {%- else -%}
          {%- set compiled_code_result = adapter.run_query_with_partitions_limit_catching(create_table_as(temporary, relation, compiled_code)) -%}
          {%- do log('COMPILED CODE RESULT: ' ~ compiled_code_result) -%}
          {%- if compiled_code_result == 'TOO_MANY_OPEN_PARTITIONS' -%}
            {%- do create_table_as_with_partitions(temporary, relation, compiled_code, language) -%}
            {%- set compiled_code_result = relation ~ ' with many partitions created' -%}
          {%- endif -%}
        {%- endif -%}
    {%- endif -%}
    {{ return(compiled_code_result) }}
{%- endmacro %}
