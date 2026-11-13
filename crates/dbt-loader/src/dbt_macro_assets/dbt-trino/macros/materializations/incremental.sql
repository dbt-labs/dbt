{% macro get_incremental_tmp_relation_type(strategy, unique_key, language) %}

  /* {#
       If we are running multiple statements (DELETE + INSERT),
       we must first save the model query results as a temporary table
       in order to guarantee consistent inputs to both statements.

       If we are running a single statement (MERGE or INSERT alone),
       we can save the model query definition as a view instead,
       for faster overall incremental processing.
  #} */
  {%- set views_enabled = config.get('views_enabled', true) -%}

  {% if language == 'sql' and (views_enabled and (strategy in ('default', 'append', 'merge') or (unique_key is none))) %}
    {{ return('view') }}
  {% else %}  {#--  play it safe -- #}
    {{ return('table') }}
  {% endif %}
{% endmacro %}

{% materialization incremental, adapter='trino', supported_languages=['sql'] -%}

  {#-- configs --#}
  {%- set unique_key = config.get('unique_key') -%}
  {%- set full_refresh_mode = (should_full_refresh()) -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}
  {%- set language = model['language'] -%}
  {%- set on_table_exists = config.get('on_table_exists', 'rename') -%}
  {% if on_table_exists not in ['rename', 'drop', 'replace'] %}
      {%- set log_message = 'Invalid value for on_table_exists (%s) specified. Setting default value (%s).' % (on_table_exists, 'rename') -%}
      {% do log(log_message) %}
      {%- set on_table_exists = 'rename' -%}
  {% endif %}
  {#-- Get the incremental_strategy and the macro to use for the strategy --#}
  {% set incremental_strategy = config.get('incremental_strategy') or 'default' %}
  {% set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) %}
  {% set strategy_sql_macro_func = adapter.get_incremental_strategy_macro(context, incremental_strategy) %}

  {#-- relations --#}
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}
  {#-- The temp relation will be a view (faster) or temp table, depending on upsert/merge strategy --#}
  {%- set tmp_relation_type = get_incremental_tmp_relation_type(incremental_strategy, unique_key, language) -%}
  {%- set tmp_relation = make_temp_relation(this).incorporate(type=tmp_relation_type) -%}
  {%- set intermediate_relation = make_intermediate_relation(target_relation) -%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}

  {#-- the temp_ and backup_ relation should not already exist in the database; get_relation
  -- will return None in that case. Otherwise, we get a relation that we can drop
  -- later, before we try to use this name for the current operation.#}
  {%- set preexisting_tmp_relation = load_cached_relation(tmp_relation)-%}
  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation)-%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}

  {#--- grab current tables grants config for comparision later on#}
  {% set grant_config = config.get('grants') %}

  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_tmp_relation) }}
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks) }}

  {% if existing_relation is none %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}

  {% elif existing_relation.is_view %}
    {#-- Can't overwrite a view with a table - we must drop --#}
    {{ log("Dropping relation " ~ target_relation ~ " because it is a view and this model is a table.") }}
    {% do adapter.drop_relation(existing_relation) %}
    {%- call statement('main', language=language) -%}
      {{ create_table_as(False, target_relation, compiled_code, language) }}
    {%- endcall -%}
  {% elif full_refresh_mode %}
    {#-- Create table with given `on_table_exists` mode #}
    {% do on_table_exists_logic(on_table_exists, existing_relation, intermediate_relation, backup_relation, target_relation) %}

  {% else %}
    {#-- Create the temp relation, either as a view or as a temp table --#}
    {% if tmp_relation_type == 'view' %}
        {%- call statement('create_tmp_relation') -%}
          {{ create_view_as(tmp_relation, compiled_code) }}
        {%- endcall -%}
    {% else %}
        {%- call statement('create_tmp_relation', language=language) -%}
          {{ create_table_as(True, tmp_relation, compiled_code, language) }}
        {%- endcall -%}
    {% endif %}

    {% do adapter.expand_target_column_types(
           from_relation=tmp_relation,
           to_relation=target_relation) %}
    {#-- Process schema changes. Returns dict of changes if successful. Use source columns for upserting/merging --#}
    {% set dest_columns = process_schema_changes(on_schema_change, tmp_relation, existing_relation) %}
    {% if not dest_columns %}
      {% set dest_columns = adapter.get_columns_in_relation(existing_relation) %}
    {% endif %}

    {#-- Build the sql --#}
    {% set strategy_arg_dict = ({'target_relation': target_relation, 'temp_relation': tmp_relation, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates }) %}
    {%- call statement('main') -%}
      {{ strategy_sql_macro_func(strategy_arg_dict) }}
    {%- endcall -%}
  {% endif %}
    {% do drop_relation_if_exists(tmp_relation) %}
  {{ run_hooks(post_hooks) }}

  {% set should_revoke =
   should_revoke(existing_relation.is_table, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}

{% macro trino__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not string %}
            delete from {{ target }}
            where exists (
                select 1
                from {{ source }}
                where
                {% for key in unique_key %}
                    {{ target }}.{{ key }} = {{ source }}.{{ key }}
                    {{ "and " if not loop.last }}
                {% endfor %}
                )
                {% if incremental_predicates %}
                    {% for predicate in incremental_predicates %}
                        and {{ predicate }}
                    {% endfor %}
                {% endif %}
            ;
        {% else %}
            delete from {{ target }}
            where (
                {{ unique_key }}) in (
                select {{ unique_key }}
                from {{ source }}
            )
            {%- if incremental_predicates %}
                {% for predicate in incremental_predicates %}
                    and {{ predicate }}
                {% endfor %}
            {%- endif -%};

        {% endif %}
    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
{%- endmacro %}

{% macro trino__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}
    {%- set predicates = [] if incremental_predicates is none else [] + incremental_predicates -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    {%- set dest_cols_csv_source = dest_cols_csv.split(', ') -%}
    {%- set merge_update_columns = config.get('merge_update_columns') -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
    {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
            {% for key in unique_key %}
                {% set this_key_match %}
                    DBT_INTERNAL_SOURCE.{{ key }} = DBT_INTERNAL_DEST.{{ key }}
                {% endset %}
                {% do predicates.append(this_key_match) %}
            {% endfor %}
        {% else %}
            {% set unique_key_match %}
                DBT_INTERNAL_SOURCE.{{ unique_key }} = DBT_INTERNAL_DEST.{{ unique_key }}
            {% endset %}
            {% do predicates.append(unique_key_match) %}
        {% endif %}

        {{ sql_header if sql_header is not none }}

        merge into {{ target }} as DBT_INTERNAL_DEST
            using {{ source }} as DBT_INTERNAL_SOURCE
            on {{"(" ~ predicates | join(") and (") ~ ")"}}

        {% if unique_key %}
        when matched then update set
            {% for column_name in update_columns -%}
                {{ column_name }} = DBT_INTERNAL_SOURCE.{{ column_name }}
                {%- if not loop.last %}, {%- endif %}
            {%- endfor %}
        {% endif %}

        when not matched then insert
            ({{ dest_cols_csv }})
        values
            ({% for dest_cols in dest_cols_csv_source -%}
                DBT_INTERNAL_SOURCE.{{ dest_cols }}
                {%- if not loop.last %}, {% endif %}
            {%- endfor %})

    {% else %}
        insert into {{ target }} ({{ dest_cols_csv }})
        (
            select {{ dest_cols_csv }}
            from {{ source }}
        )
    {% endif %}
{% endmacro %}


{% macro trino__get_incremental_microbatch_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}

    {#-- Add additional incremental_predicates to filter for batch --#}
    {% if model.config.get("__dbt_internal_microbatch_event_time_start") -%}
        {% do incremental_predicates.append(model.config.event_time ~ " >= TIMESTAMP '" ~ model.config.__dbt_internal_microbatch_event_time_start ~ "'") %}
    {% endif %}
    {% if model.config.get("__dbt_internal_microbatch_event_time_end") -%}
        {% do incremental_predicates.append(model.config.event_time ~ " < TIMESTAMP '" ~ model.config.__dbt_internal_microbatch_event_time_end ~ "'") %}
    {% endif %}
    {% do arg_dict.update({'incremental_predicates': incremental_predicates}) %}

    delete from {{ target }}
    where (
    {% for predicate in incremental_predicates %}
        {%- if not loop.first %}and {% endif -%} {{ predicate }}
    {% endfor %}
    );

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
{% endmacro %}
