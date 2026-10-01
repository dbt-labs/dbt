-- TODO create a drop_relation_with_versions, to be sure to remove all historical versions of a table
{% materialization table, adapter='athena', supported_languages=['sql', 'python'] -%}
  {%- set identifier = model['alias'] -%}
  {%- set language = model['language'] -%}

  {%- set lf_tags_config = config.get('lf_tags_config') -%}
  {%- set lf_grants = config.get('lf_grants') -%}

  {%- set table_type = resolve_table_type() -%}
  {%- set old_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) -%}
  {%- set old_tmp_relation = adapter.get_relation(identifier=identifier ~ '__ha',
                                             schema=schema,
                                             database=database) -%}
  {%- set old_bkp_relation = adapter.get_relation(identifier=identifier ~ '__bkp',
                                             schema=schema,
                                             database=database) -%}
  {%- set is_ha = config.get('ha', default=false) -%}
  {%- set s3_data_dir = config.get('s3_data_dir', default=target.s3_data_dir) -%}
  {%- set s3_data_naming = config.get('s3_data_naming', default='table_unique') -%}
  {%- set full_refresh_config = config.get('full_refresh', default=False) -%}
  {%- set is_full_refresh_mode = (flags.FULL_REFRESH == True or full_refresh_config == True) -%}
  {%- set versions_to_keep = config.get('versions_to_keep', default=4) -%}
  {%- set external_location = config.get('external_location', default=none) -%}
  {%- set force_batch = config.get('force_batch', False) | as_bool -%}
  {%- set target_relation = api.Relation.create(identifier=identifier,
                                                schema=schema,
                                                database=database,
                                                type='table') -%}
  {%- set tmp_relation = api.Relation.create(identifier=target_relation.identifier ~ '__ha',
                                             schema=schema,
                                             database=database,
                                             s3_path_table_part=target_relation.identifier,
                                             type='table') -%}

  {%- if (
    table_type == 'hive'
    and is_ha
    and ('unique' not in s3_data_naming or external_location is not none)
  ) -%}
      {%- set error_unique_location_hive_ha -%}
          You need to have an unique table location when using ha config with hive table.
          Use s3_data_naming unique, table_unique or schema_table_unique, and avoid to set an explicit
          external_location.
      {%- endset -%}
      {% do exceptions.raise_compiler_error(error_unique_location_hive_ha) %}
  {%- endif -%}

  {{ run_hooks(pre_hooks) }}

  {%- if table_type == 'hive' -%}

    -- for ha tables that are not in full refresh mode and when the relation exists we use the swap behavior
    {%- if is_ha and not is_full_refresh_mode and old_relation is not none -%}
      -- drop the old_tmp_relation if it exists
      {%- if old_tmp_relation is not none -%}
        {%- do adapter.delete_from_glue_catalog(old_tmp_relation) -%}
      {%- endif -%}

      -- create tmp table
      {%- set query_result = safe_create_table_as(False, tmp_relation, compiled_code, language, force_batch) -%}
      -- Execute python code that is available in query result object
      {%- if language == 'python' -%}
        {% call statement('create_table', language=language) %}
          {{ query_result }}
        {% endcall %}
      {%- endif -%}
      -- swap table
      {%- set swap_table = adapter.swap_table(tmp_relation, target_relation) -%}

      -- delete glue tmp table, do not use drop_relation, as it will remove data of the target table
      {%- do adapter.delete_from_glue_catalog(tmp_relation) -%}

      {% do adapter.expire_glue_table_versions(target_relation, versions_to_keep, True) %}

    {%- else -%}
      -- Here we are in the case of non-ha tables or ha tables but in case of full refresh.
      {%- if old_relation is not none -%}
        {{ drop_relation(old_relation) }}
      {%- endif -%}
      {%- set query_result = safe_create_table_as(False, target_relation, compiled_code, language, force_batch) -%}
      -- Execute python code that is available in query result object
      {%- if language == 'python' -%}
        {% call statement('create_table', language=language) %}
          {{ query_result }}
        {% endcall %}
      {%- endif -%}
    {%- endif -%}

    {%- if language != 'python' -%}
      {{ set_table_classification(target_relation) }}
    {%- endif -%}
  {%- else -%}

    {%- if old_relation is none -%}
      {%- set query_result = safe_create_table_as(False, target_relation, compiled_code, language, force_batch) -%}
      -- Execute python code that is available in query result object
      {%- if language == 'python' -%}
        {% call statement('create_table', language=language) %}
          {{ query_result }}
        {% endcall %}
      {%- endif -%}
    {%- elif adapter.is_s3_tables_database(database) -%}
      -- S3 Tables manages its own storage and does not support ALTER TABLE RENAME, so the
      -- near-zero-downtime swap used for Glue Iceberg is not possible. Drop the existing
      -- table (via SQL) and recreate it directly.
      {%- do drop_relation(old_relation) -%}
      {%- set query_result = safe_create_table_as(False, target_relation, compiled_code, language, force_batch) -%}
      {%- if language == 'python' -%}
        {% call statement('create_table', language=language) %}
          {{ query_result }}
        {% endcall %}
      {%- endif -%}
    {%- else -%}
      {%- if old_relation.is_view -%}
        {%- set query_result = safe_create_table_as(False, tmp_relation, compiled_code, language, force_batch) -%}
        -- Execute python code that is available in query result object
        {%- if language == 'python' -%}
          {% call statement('create_table', language=language) %}
            {{ query_result }}
          {% endcall %}
        {%- endif -%}
        {%- do drop_relation(old_relation) -%}
        {%- do rename_relation(tmp_relation, target_relation) -%}
      {%- else -%}
        -- delete old tmp iceberg table if it exists
        {%- if old_tmp_relation is not none -%}
          {%- do drop_relation(old_tmp_relation) -%}
        {%- endif -%}

        -- If we have this, it means that at least the first renaming occurred but there was an issue
        -- afterwards, therefore we are in weird state. The easiest and cleanest should be to remove
        -- the backup relation. It won't have an impact because since we are in the else condition,
        -- that means that old relation exists therefore no downtime yet.
        {%- if old_bkp_relation is not none -%}
          {%- do drop_relation(old_bkp_relation) -%}
        {%- endif -%}

        {% set query_result = safe_create_table_as(False, tmp_relation, compiled_code, language, force_batch) %}
        -- Execute python code that is available in query result object
        {%- if language == 'python' -%}
          {% call statement('create_table', language=language) %}
            {{ query_result }}
          {% endcall %}
        {%- endif -%}

        {%- set old_relation_table_type = adapter.get_glue_table_type(old_relation).value if old_relation else none -%}

        -- we cannot use old_bkp_relation, because it returns None if the relation doesn't exist
        -- we need to create a python object via the make_temp_relation instead
        {%- set old_relation_bkp = make_temp_relation(old_relation, '__bkp') -%}

        {%- if old_relation_table_type == 'iceberg_table' -%}
          {{ rename_relation(old_relation, old_relation_bkp) }}
        {%- else  -%}
          {%- do drop_relation_glue(old_relation) -%}
        {%- endif -%}

        -- publish the target table doing a final renaming
        {{ rename_relation(tmp_relation, target_relation) }}

        -- if old relation is iceberg_table, we have a backup
        -- therefore we can drop the old relation backup, in all other cases there is nothing to do
        -- in case of switch from hive to iceberg the backup table do not exists
        -- in case of first run, the backup table do not exists
        {%- if old_relation_table_type == 'iceberg_table' -%}
          {%- do drop_relation(old_relation_bkp) -%}
        {%- endif -%}

      {%- endif -%}
    {%- endif -%}

  {%- endif -%}

  {% call statement("main", language=language) %}
    {%- if language=='sql' -%}
      SELECT '{{ query_result }}';
    {%- endif -%}
  {% endcall %}

  {{ run_hooks(post_hooks) }}

  {% if lf_tags_config is not none %}
    {{ adapter.add_lf_tags(target_relation, lf_tags_config) }}
  {% endif %}

  {% if lf_grants is not none %}
    {{ adapter.apply_lf_grants(target_relation, lf_grants) }}
  {% endif %}

  {% do persist_docs(target_relation, model) %}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
