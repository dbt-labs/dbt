{% macro get_incremental_strategy(file_format) %}
  {% set raw_strategy = config.get('incremental_strategy') or 'merge' %}
  {% do return(dbt_databricks_validate_get_incremental_strategy(raw_strategy, file_format)) %}
{% endmacro %}

{% macro databricks__get_incremental_default_sql(arg_dict) %}
  {{ return(get_incremental_merge_sql(arg_dict)) }}
{% endmacro %}

{% macro databricks__get_incremental_append_sql(arg_dict) %}
  {% do return(get_insert_into_sql(arg_dict["temp_relation"], arg_dict["target_relation"])) %}
{% endmacro %}

{% macro databricks__get_incremental_replace_where_sql(arg_dict) %}
  {% do return(get_replace_where_sql(arg_dict)) %}
{% endmacro %}

{% macro databricks__get_incremental_delete_insert_sql(arg_dict) %}
  {% do return(get_delete_insert_sql(arg_dict)) %}
{% endmacro %}

{% macro get_incremental_replace_where_sql(arg_dict) %}

  {{ return(adapter.dispatch('get_incremental_replace_where_sql', 'dbt')(arg_dict)) }}

{% endmacro %}

{% macro databricks__get_insert_overwrite_merge_sql(target, source, dest_columns, predicates, include_sql_header) %}
    {{ return(get_insert_overwrite_sql(source, target)) }}
{% endmacro %}

{% macro get_insert_overwrite_sql(source_relation, target_relation) %}
    {%- if adapter.is_cluster() -%}
        {#-- Clusters: Check DBR capability for REPLACE ON (DBR 17.1+) --#}
        {%- if adapter.has_dbr_capability('replace_on') -%}
            {{ get_insert_replace_on_sql(source_relation, target_relation) }}
        {%- else -%}
            {%- if adapter.behavior.use_replace_on_for_insert_overwrite -%}
                {{ exceptions.warn("insert_overwrite: use_replace_on_for_insert_overwrite is enabled but this cluster's DBR version does not support REPLACE ON (requires DBR 17.1+). Falling back to legacy INSERT OVERWRITE.") }}
            {%- endif -%}
            {%- set has_insert_by_name = adapter.has_dbr_capability('insert_by_name') -%}
            insert overwrite table {{ target_relation }}
            {{ partition_cols(label="partition") }}
            {%- if has_insert_by_name %} by name{% endif %}
            select * from {{ source_relation }}
        {%- endif -%}
    {%- else -%}
        {%- if adapter.behavior.use_replace_on_for_insert_overwrite -%}
            {{ get_insert_replace_on_sql(source_relation, target_relation) }}
        {%- else -%}
            {#-- Behavior flag disabled, use legacy DPO INSERT OVERWRITE --#}
            {%- set has_insert_by_name = adapter.has_dbr_capability('insert_by_name') -%}
            insert overwrite table {{ target_relation }}
            {{ partition_cols(label="partition") }}
            {%- if has_insert_by_name %} by name{% endif %}
            select * from {{ source_relation }}
        {%- endif -%}
    {%- endif -%}
{% endmacro %}

{% macro add_columns_to_list(target_list, columns) %}
    {#-- Local helper to add columns to the target list, converting string to list if needed --#}
    {%- if columns -%}
        {%- if columns is string -%}
            {%- do target_list.append(columns) -%}
        {%- else -%}
            {%- for col in columns -%}
                {%- do target_list.append(col) -%}
            {%- endfor -%}
        {%- endif -%}
    {%- endif -%}
{% endmacro %}

{% macro get_insert_replace_on_sql(source_relation, target_relation) %}
    {%- set partition_by = config.get('partition_by') -%}
    {%- set liquid_clustered_by = config.get('liquid_clustered_by') -%}
    {%- set replace_columns = [] -%}
    
    {#-- If both partition_by and liquid_clustered_by are defined, it will fail before this point with a SPECIFY_CLUSTER_BY_WITH_PARTITIONED_BY_IS_NOT_ALLOWED error from Databricks --#}
    {%- do add_columns_to_list(replace_columns, partition_by) -%}
    {%- do add_columns_to_list(replace_columns, liquid_clustered_by) -%}
    
    {%- if replace_columns -%}
        {%- set replace_conditions = [] -%}
        {%- for col in replace_columns -%}
            {%- do replace_conditions.append('t.' ~ adapter.quote(col) ~ ' <=> s.' ~ adapter.quote(col)) -%}
        {%- endfor -%}
        {%- set replace_conditions_csv = replace_conditions | join(' AND ') -%}
        {%- set source_columns = adapter.get_columns_in_relation(source_relation) | map(attribute="name") | list -%}
        {%- set dest_columns = adapter.get_columns_in_relation(target_relation) | map(attribute="name") | list -%}
        {%- set source_cols_lower = source_columns | map('lower') | list -%}
        {%- set select_columns = [] -%}
        {%- for dest_col in dest_columns -%}
            {%- set dest_col_lower = dest_col | lower -%}
            {%- set matched_col = namespace(value=none) -%}
            {%- for src_col in source_columns -%}
                {%- if src_col | lower == dest_col_lower and matched_col.value is none -%}
                    {%- set matched_col.value = src_col -%}
                {%- endif -%}
            {%- endfor -%}
            {%- if matched_col.value is not none -%}
                {%- do select_columns.append(adapter.quote(matched_col.value)) -%}
            {%- else -%}
                {%- do select_columns.append('NULL as ' ~ adapter.quote(dest_col)) -%}
            {%- endif -%}
        {%- endfor -%}
        insert into table {{ target_relation }} AS t
        replace on ({{ replace_conditions_csv }})
        (select {{ select_columns | join(', ') }} from {{ source_relation }}) AS s
    {%- else -%}
        {#-- Fallback to regular insert overwrite if no partitioning nor liquid clustering defined --#}
        {%- set has_insert_by_name = adapter.has_dbr_capability('insert_by_name') -%}
        insert overwrite table {{ target_relation }}
        {%- if has_insert_by_name %} by name{% endif %}
        select * from {{ source_relation }}
    {%- endif -%}
{% endmacro %}

{% macro get_replace_where_sql(args_dict) -%}
  {%- set predicates = args_dict['incremental_predicates'] -%}
  {%- set target_relation = args_dict['target_relation'] -%}
  {%- set temp_relation = args_dict['temp_relation'] -%}
  {#-- BY NAME + REPLACE WHERE needs DBR 18.0+ on clusters (SPARK-54803), a higher floor than
       plain insert_by_name; emitting it on older clusters fails to parse (issue #1532). --#}
  {%- set has_by_name = adapter.has_dbr_capability('insert_by_name_replace_where') -%}
INSERT INTO {{ target_relation.render() }}
{%- if has_by_name %} BY NAME{% endif %}
{%- if predicates %}
  {%- if predicates is sequence and predicates is not string %}
 REPLACE WHERE {{ predicates | join(' and ') }}
  {%- else %}
 REPLACE WHERE {{ predicates }}
  {%- endif %}
{%- endif %}
 TABLE {{ temp_relation.render() }}
{%- endmacro %}

{% macro get_delete_insert_sql(arg_dict) -%}
  {%- set source_relation = arg_dict.get('temp_relation') -%}
  {%- set target_relation = arg_dict.get('target_relation') -%}
  {%- set incremental_predicates = config.get('incremental_predicates') -%}
  {%- set target_columns = (adapter.get_columns_in_relation(target_relation) | map(attribute='quoted') | list) -%}
  {%- set unique_key = config.require('unique_key') -%}
  {% do return(delete_insert_sql_impl(source_relation, target_relation, target_columns, unique_key, incremental_predicates)) %}
{% endmacro %}

{% macro delete_insert_sql_impl(source_relation, target_relation, target_columns, unique_key, incremental_predicates) %}
  {# we use target_columns to select from source to avoid potential column order issues during insert #}
  {%- set target_cols_csv = target_columns | join(', ') -%}
  {%- set predicates -%}
    {%- if incremental_predicates is sequence and incremental_predicates is not string -%}
      where {{ incremental_predicates | join(' and ') }}
    {%- elif incremental_predicates is string and incremental_predicates is not none -%}
      where {{ incremental_predicates }}
    {%- endif -%}
  {%- endset -%}
  {%- set unique_keys = unique_key if unique_key is sequence and unique_key is not string else [unique_key] -%}
  
  {%- if adapter.has_dbr_capability('replace_on') -%}
    {#-- DBR 17.1+: Use efficient REPLACE ON syntax --#}
    {%- set replace_on_expr = [] -%}
    {%- for key in unique_keys -%}
      {%- do replace_on_expr.append('target.' ~ adapter.quote(key) ~ ' <=> temp.' ~ adapter.quote(key)) -%}
    {%- endfor -%}
    {%- set replace_on_expr = replace_on_expr | join(' and ') -%}
 insert into table {{ target_relation }} as target
replace on ({{ replace_on_expr }})
(select {{ target_cols_csv }}
   from {{ source_relation }} {{ predicates }}) as temp
  {%- else -%}
    {#-- DBR < 17.1: Fallback to DELETE FROM + INSERT INTO --#}
    {% do return(delete_insert_legacy_sql(source_relation, target_relation, target_columns, unique_keys, incremental_predicates)) %}
  {%- endif -%}
{% endmacro %}

{% macro delete_insert_legacy_sql(source_relation, target_relation, target_columns, unique_keys, incremental_predicates) %}
  {#-- Legacy implementation for DBR < 17.1 using DELETE FROM + INSERT INTO --#}
  {#-- Returns a list of SQL statements to be executed via execute_multiple_statements --#}
  {%- set target_cols_csv = target_columns | join(', ') -%}
  {%- set statements = [] -%}
  
  {#-- Build WHERE clause for DELETE statement --#}
  {%- set delete_conditions = [] -%}
  {%- for key in unique_keys -%}
    {%- do delete_conditions.append(target_relation ~ '.' ~ adapter.quote(key) ~ ' IN (SELECT ' ~ adapter.quote(key) ~ ' FROM ' ~ source_relation ~ ')') -%}
  {%- endfor -%}
  
  {#-- Add incremental predicates to DELETE if specified --#}
  {%- if incremental_predicates is sequence and incremental_predicates is not string -%}
    {%- for predicate in incremental_predicates -%}
      {%- do delete_conditions.append(predicate) -%}
    {%- endfor -%}
  {%- elif incremental_predicates is string and incremental_predicates is not none -%}
    {%- do delete_conditions.append(incremental_predicates) -%}
  {%- endif -%}
  
  {#-- Step 1: DELETE matching rows --#}
  {%- set delete_sql -%}
delete from {{ target_relation }}
where {{ delete_conditions | join('\n  and ') }}
  {%- endset -%}
  {%- do statements.append(delete_sql) -%}

  {#-- Step 2: INSERT new rows --#}
  {%- set has_insert_by_name = adapter.has_dbr_capability('insert_by_name') -%}
  {%- set insert_sql -%}
insert into {{ target_relation }}{% if has_insert_by_name %} by name{% endif %}

select {{ target_cols_csv }}
from {{ source_relation }}
    {%- if incremental_predicates is sequence and incremental_predicates is not string %}
where {{ incremental_predicates | join(' and ') }}
    {%- elif incremental_predicates is string and incremental_predicates is not none %}
where {{ incremental_predicates }}
    {%- endif %}
  {%- endset -%}
  {%- do statements.append(insert_sql) -%}
  
  {% do return(statements) %}
{% endmacro %}


{% macro get_insert_into_sql(source_relation, target_relation) %}
    {%- set source_columns = adapter.get_columns_in_relation(source_relation) | map(attribute="name") | list -%}
    {%- set dest_columns = adapter.get_columns_in_relation(target_relation) | map(attribute="name") | list -%}
    {{ insert_into_sql_impl(target_relation, dest_columns, source_relation, source_columns) }}
{% endmacro %}

{% macro insert_into_sql_impl(target_relation, dest_columns, source_relation, source_columns) %}
    {%- set dest_cols_lower = dest_columns | map('lower') | list -%}
    {%- set source_cols_lower = source_columns | map('lower') | list -%}
    {%- set has_insert_by_name = adapter.has_dbr_capability('insert_by_name') -%}

    {%- if dest_cols_lower | sort == source_cols_lower | sort -%}
        {#-- All columns match (case-insensitive) --#}
        {%- if has_insert_by_name -%}
            {#-- DBR 12.2+: Use INSERT BY NAME for name-based matching --#}
            insert into {{ target_relation }} by name
            select * from {{ source_relation }}
        {%- else -%}
            {#-- DBR < 12.2: Use positional INSERT (columns must be in same order) --#}
            insert into {{ target_relation }}
            select * from {{ source_relation }}
        {%- endif -%}
    {%- else -%}
        {#-- Columns don't match, select only columns that exist in target --#}
        {#-- Note: Cannot use BY NAME with explicit column list, so use traditional syntax --#}
        {%- set common_columns = [] -%}
        {%- for dest_col in dest_columns -%}
            {%- if dest_col | lower in source_cols_lower -%}
                {%- do common_columns.append(adapter.quote(dest_col)) -%}
            {%- endif -%}
        {%- endfor -%}
        {%- if common_columns | length > 0 -%}
            insert into {{ target_relation }} ({{ common_columns | join(', ') }})
            select {{ common_columns | join(', ') }} from {{ source_relation }}
        {%- else -%}
            {#-- No common columns, this shouldn't happen but handle it gracefully --#}
            {%- if has_insert_by_name -%}
                insert into {{ target_relation }} by name
                select * from {{ source_relation }}
            {%- else -%}
                insert into {{ target_relation }}
                select * from {{ source_relation }}
            {%- endif -%}
        {%- endif -%}
    {%- endif -%}
{%- endmacro %}

{% macro databricks__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
  {# need dest_columns for merge_exclude_columns, default to use "*" #}

  {%- set target_alias = config.get('target_alias', 'DBT_INTERNAL_DEST') -%}
  {%- set source_alias = config.get('source_alias', 'DBT_INTERNAL_SOURCE') -%}

  {%- set predicates = [] if incremental_predicates is none else [] + incremental_predicates -%}
  {%- set dest_columns = adapter.get_columns_in_relation(target) -%}
  {%- set source_columns = (adapter.get_columns_in_relation(source) | map(attribute='name') | list)-%}
  {%- set merge_update_columns = config.get('merge_update_columns') -%}
  {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
  {%- set merge_with_schema_evolution = (config.get('merge_with_schema_evolution') | lower == 'true') -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}
  {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) -%}
  {%- set skip_matched_step = (config.get('skip_matched_step') | lower == 'true') -%}
  {%- set skip_not_matched_step = (config.get('skip_not_matched_step') | lower == 'true') -%}

  {%- set matched_condition = config.get('matched_condition') -%}
  {%- set not_matched_condition = config.get('not_matched_condition') -%}

  {%- set not_matched_by_source_action = config.get('not_matched_by_source_action') -%}
  {%- set not_matched_by_source_condition = config.get('not_matched_by_source_condition') -%}

  {%- set not_matched_by_source_action_trimmed = not_matched_by_source_action | lower | trim(' \n\t') %}
  {%- set not_matched_by_source_action_is_set = (
      not_matched_by_source_action_trimmed == 'delete'
      or not_matched_by_source_action_trimmed.startswith('update')
    )
  %}
  
  
  {% if unique_key %}
      {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
          {% for key in unique_key %}
              {% set this_key_match %}
                  {{ source_alias }}.{{ adapter.quote(key) }} <=> {{ target_alias }}.{{ adapter.quote(key) }}
              {% endset %}
              {% do predicates.append(this_key_match) %}
          {% endfor %}
      {% else %}
          {% set unique_key_match %}
              {{ source_alias }}.{{ adapter.quote(unique_key) }} <=> {{ target_alias }}.{{ adapter.quote(unique_key) }}
          {% endset %}
          {% do predicates.append(unique_key_match) %}
      {% endif %}
  {% else %}
      {% do predicates.append('FALSE') %}
  {% endif %}

    merge
        {%- if merge_with_schema_evolution %}
        with schema evolution
        {%- endif %}
    into
        {{ target }} as {{ target_alias }}
    using
        {{ source }} as {{ source_alias }}
    on
        {{ predicates | join('\n    and ') }}
    {%- if not skip_matched_step %}
    when matched
        {%- if matched_condition %}
        and ({{ matched_condition }})
        {%- endif %}
        then update set
            {{ get_merge_update_set(update_columns, on_schema_change, source_columns, source_alias) }}
    {%- endif %}
    {%- if not skip_not_matched_step %}
    when not matched
        {%- if not_matched_condition %}
        and ({{ not_matched_condition }})
        {%- endif %}
        then insert
            {{ get_merge_insert(on_schema_change, source_columns, source_alias) }}
    {%- endif %}
    {%- if not_matched_by_source_action_is_set %}
    when not matched by source
        {%- if not_matched_by_source_condition %}
        and ({{ not_matched_by_source_condition }})
        {%- endif %}
        then {{ not_matched_by_source_action }}
    {%- endif %}
{% endmacro %}

{% macro get_merge_update_set(update_columns, on_schema_change, source_columns, source_alias='DBT_INTERNAL_SOURCE') %}
  {%- if update_columns -%}
    {%- for column_name in update_columns -%}
      {{ adapter.quote(column_name) }} = {{ source_alias }}.{{ adapter.quote(column_name) }}{%- if not loop.last %}, {% endif -%}
    {%- endfor %}
  {%- elif on_schema_change == 'ignore' -%}
    *
  {%- else -%}
    {%- for column in source_columns -%}
      {{ adapter.quote(column) }} = {{ source_alias }}.{{ adapter.quote(column) }}{%- if not loop.last %}, {% endif -%}
    {%- endfor %}
  {%- endif -%}
{% endmacro %}

{% macro get_merge_insert(on_schema_change, source_columns, source_alias='DBT_INTERNAL_SOURCE') %}
  {%- if on_schema_change == 'ignore' -%}
    *
  {%- else -%}
    ({% for column in source_columns %}{{ adapter.quote(column) }}{% if not loop.last %}, {% endif %}{% endfor %}) VALUES (
    {%- for column in source_columns -%}
      {{ source_alias }}.{{ adapter.quote(column) }}{%- if not loop.last %}, {% endif -%}
    {%- endfor %})
  {%- endif -%}
{% endmacro %}

{% macro databricks__get_incremental_microbatch_sql(arg_dict) %}
  {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}
  {%- set event_time = model.config.event_time -%}
  {%- set start_time = config.get("__dbt_internal_microbatch_event_time_start") -%}
  {%- set end_time = config.get("__dbt_internal_microbatch_event_time_end") -%}
  {%- if start_time -%}
    {%- do incremental_predicates.append("cast(" ~ event_time ~ " as TIMESTAMP) >= '" ~ start_time ~ "'") -%}
  {%- endif -%}
  {%- if end_time -%}
    {%- do incremental_predicates.append("cast(" ~ event_time ~ " as TIMESTAMP) < '" ~ end_time ~ "'") -%}
  {%- endif -%}
  {%- do arg_dict.update({'incremental_predicates': incremental_predicates}) -%}
  {{ return(get_replace_where_sql(arg_dict)) }}
{% endmacro %}



{% macro databricks__get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) %}
  {%- set default_cols = None -%}

  {%- if merge_update_columns and merge_exclude_columns -%}
    {{ exceptions.raise_compiler_error(
        'Model cannot specify merge_update_columns and merge_exclude_columns. Please update model to use only one config'
    )}}
  {%- elif merge_update_columns -%}
    {# Don't quote here - columns will be quoted in get_merge_update_set #}
    {%- set update_columns = merge_update_columns -%}
  {%- elif merge_exclude_columns -%}
    {%- set update_columns = [] -%}
    {%- for column in dest_columns -%}
      {% if column.column | lower not in merge_exclude_columns | map("lower") | list %}
        {%- do update_columns.append(column.column) -%}
      {% endif %}
    {%- endfor -%}
  {%- else -%}
    {%- set update_columns = default_cols -%}
  {%- endif -%}

  {{ return(update_columns) }}

{% endmacro %}
