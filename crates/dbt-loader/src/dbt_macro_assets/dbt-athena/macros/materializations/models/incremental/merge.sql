{% macro get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) %}

  {%- if merge_update_columns and merge_exclude_columns -%}
    {{ exceptions.raise_compiler_error(
        'Model cannot specify merge_update_columns and merge_exclude_columns. Please update model to use only one config'
    )}}
  {%- elif merge_update_columns -%}
    {%- set update_columns = [] -%}
    {%- for column in dest_columns -%}
      {% if column.column | lower in merge_update_columns | map("lower") | list %}
        {%- do update_columns.append(column) -%}
      {% endif %}
    {%- endfor -%}
  {%- elif merge_exclude_columns -%}
    {%- set update_columns = [] -%}
    {%- for column in dest_columns -%}
      {% if column.column | lower not in merge_exclude_columns | map("lower") | list %}
        {%- do update_columns.append(column) -%}
      {% endif %}
    {%- endfor -%}
  {%- else -%}
    {%- set update_columns = dest_columns -%}
  {%- endif -%}

  {{ return(update_columns) }}

{% endmacro %}

{%- macro get_update_statement(col, rule, is_last) -%}
    {%- if rule == "coalesce" -%}
        {{ col.quoted }} = {{ 'coalesce(src.' + col.quoted + ', target.' + col.quoted + ')' }}
    {%- elif rule == "sum" -%}
      {%- if col.data_type.startswith("map") -%}
          {{ col.quoted }} = {{ 'map_zip_with(coalesce(src.' + col.quoted + ', map()), coalesce(target.' + col.quoted + ', map()), (k, v1, v2) -> coalesce(v1, 0) + coalesce(v2, 0))' }}
      {%- else -%}
        {{ col.quoted }} = {{ 'src.' + col.quoted + ' + target.' + col.quoted }}
      {%- endif -%}
    {%- elif rule == "append" -%}
        {{ col.quoted }} = {{ 'src.' + col.quoted + ' || target.' + col.quoted }}
    {%- elif rule == "append_distinct" -%}
        {{ col.quoted }} = {{ 'array_distinct(src.' + col.quoted + ' || target.' + col.quoted + ')' }}
    {%- elif rule == "replace" -%}
        {{ col.quoted }} = {{ 'src.' + col.quoted }}
    {%- else -%}
        {{ col.quoted }} = {{ rule | replace("_new_", 'src.' + col.quoted) | replace("_old_", 'target.' + col.quoted) }}
    {%- endif -%}
    {{ "," if not is_last }}
{%- endmacro -%}


{% macro batch_iceberg_merge(tmp_relation, target_relation, merge_part, dest_cols_csv) %}
    {% set partitions_batches = get_partition_batches(tmp_relation) %}
    {% do log('BATCHES TO PROCESS: ' ~ partitions_batches | length) %}
    {%- for batch in partitions_batches -%}
        {%- do log('BATCH PROCESSING: ' ~ loop.index ~ ' OF ' ~ partitions_batches | length) -%}
        {%- set src_batch_part -%}
            merge into {{ target_relation }} as target
            using (select {{ dest_cols_csv }} from {{ tmp_relation }} where {{ batch }}) as src
        {%- endset -%}
        {%- set merge_batch -%}
          {{ src_batch_part }}
          {{ merge_part }}
        {%- endset -%}
        {%- do run_query(merge_batch) -%}
    {%- endfor -%}
{%- endmacro -%}


{% macro iceberg_merge(
    on_schema_change,
    tmp_relation,
    target_relation,
    unique_key,
    incremental_predicates,
    existing_relation,
    delete_condition,
    update_condition,
    insert_condition,
    force_batch,
    statement_name="main"
  )
%}
    {%- set merge_update_columns = config.get('merge_update_columns') -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
    {%- set merge_update_columns_default_rule = config.get('merge_update_columns_default_rule', 'replace') -%}
    {%- set merge_update_columns_rules = config.get('merge_update_columns_rules') -%}

    {% set dest_columns = process_schema_changes(on_schema_change, tmp_relation, existing_relation) %}
    {% if not dest_columns %}
      {%- set dest_columns = adapter.get_columns_in_relation(target_relation) -%}
    {% endif %}
    {%- set dest_cols_csv = dest_columns | map(attribute='quoted') | join(', ') -%}
    {%- if unique_key is sequence and unique_key is not string -%}
      {%- set unique_key_cols = unique_key -%}
    {%- else -%}
      {%- set unique_key_cols = [unique_key] -%}
    {%- endif -%}
    {%- set src_columns_quoted = [] -%}
    {%- set dest_columns_wo_keys = [] -%}
    {%- for col in dest_columns -%}
      {%- do src_columns_quoted.append('src.' + col.quoted ) -%}
      {%- if col.name not in unique_key_cols -%}
        {%- do dest_columns_wo_keys.append(col) -%}
      {%- endif -%}
    {%- endfor -%}
    {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns_wo_keys) -%}
    {%- set src_cols_csv = src_columns_quoted | join(', ') -%}

    {%- set merge_part -%}
      on (
          {%- for key in unique_key_cols -%}
            target.{{ key }} = src.{{ key }}
            {{ " and " if not loop.last }}
          {%- endfor -%}
          {% if incremental_predicates is not none -%}
          and (
            {%- for inc_predicate in incremental_predicates %}
              {{ inc_predicate }} {{ "and " if not loop.last }}
          {%- endfor %}
          )
          {%- endif %}
      )
      {% if delete_condition is not none -%}
          when matched and ({{ delete_condition }})
          then delete
      {%- endif %}
      {% if update_columns -%}
        when matched {% if update_condition is not none -%} and {{ update_condition }} {%- endif %}
          then update set
            {%- for col in update_columns %}
              {%- if merge_update_columns_rules and col.name in merge_update_columns_rules %}
                {{ get_update_statement(col, merge_update_columns_rules[col.name], loop.last) }}
              {%- else -%}
                {{ get_update_statement(col, merge_update_columns_default_rule, loop.last) }}
              {%- endif -%}
            {%- endfor %}
      {%- endif %}
      when not matched {% if insert_condition is not none -%} and {{ insert_condition }} {%- endif %}
        then insert ({{ dest_cols_csv }})
         values ({{ src_cols_csv }})
    {%- endset -%}

    {%- if force_batch -%}
      {% do batch_iceberg_merge(tmp_relation, target_relation, merge_part, dest_cols_csv) %}
    {%- else -%}
      {%- set src_part -%}
          merge into {{ target_relation }} as target using {{ tmp_relation }} as src
      {%- endset -%}
      {%- set merge_full -%}
          {{ src_part }}
          {{ merge_part }}
      {%- endset -%}

      {%- set query_result =  adapter.run_query_with_partitions_limit_catching(merge_full) -%}
      {%- do log('QUERY RESULT: ' ~ query_result) -%}
      {%- if query_result == 'TOO_MANY_OPEN_PARTITIONS' -%}
        {% do batch_iceberg_merge(tmp_relation, target_relation, merge_part, dest_cols_csv) %}
      {%- endif -%}
    {%- endif -%}

    SELECT '{{query_result}}'
{%- endmacro %}
