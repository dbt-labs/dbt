{% materialization incremental, adapter='singlestore' -%}

  -- relations
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set target_relation = this.incorporate(type='table') -%}
  {%- set temp_relation = make_temp_relation(target_relation)-%}
  {%- set intermediate_relation = make_intermediate_relation(target_relation)-%}
  {%- set backup_relation_type = 'table' if existing_relation is none else existing_relation.type -%}
  {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}

  -- configs
  {%- set unique_key = config.get('unique_key') -%}
  {%- set full_refresh_mode = (should_full_refresh() or existing_relation.is_view) -%}
  {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}

  {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation)-%}
  {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
  {% set grant_config = config.get('grants') %}
  {{ drop_stale_temp_and_backup_relations(target_relation) }}
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {% set to_drop = [] %}

  {% set incremental_strategy = config.get('incremental_strategy') or 'default' %}
  {% set strategy_sql_macro_func = adapter.get_incremental_strategy_macro(context, incremental_strategy) %}

  {%- if incremental_strategy == 'merge' and not unique_key -%}
    {%- set model_name = model.name if model is defined and model.name is defined else target_relation.identifier -%}
    {{ exceptions.raise_compiler_error(
      "Incremental model '" ~ model_name ~ "' configured with incremental_strategy='merge' requires a 'unique_key' configuration and a matching PRIMARY KEY or UNIQUE constraint on the target table. Fix: specify 'unique_key' and define a matching primary_key or unique_table_key constraint."
    ) }}
  {%- endif -%}

  {% if existing_relation is none %}
      {% set build_sql = get_create_table_as_sql(False, target_relation, sql) %}
      {% set relation_for_indexes = target_relation %}
  {% elif full_refresh_mode %}
      {% set build_sql = get_create_table_as_sql(False, intermediate_relation, sql) %}
      {% set relation_for_indexes = intermediate_relation %}
      {% set need_swap = true %}
  {% else %}
    {% do run_query(get_create_table_as_sql(True, temp_relation, sql)) %}
    {% do to_drop.append(temp_relation) %}
    {% set relation_for_indexes = temp_relation %}
    {% set contract_config = config.get('contract') %}
    {% if not contract_config or not contract_config.enforced %}
      {% do adapter.expand_target_column_types(
               from_relation=temp_relation,
               to_relation=target_relation) %}
    {% endif %}
    {% set dest_columns = process_schema_changes(on_schema_change, temp_relation, existing_relation) %}
    {% if not dest_columns %}
      {% set dest_columns = adapter.get_columns_in_relation(existing_relation) %}
    {% endif %}

    {% set incremental_predicates = config.get('predicates', none) or config.get('incremental_predicates', none) %}
    {% set strategy_arg_dict = ({'target_relation': target_relation, 'temp_relation': temp_relation, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates }) %}
    {% set build_sql = strategy_sql_macro_func(strategy_arg_dict) %}

  {% endif %}

  {% call statement("main") %}
      {{ build_sql }}
  {% endcall %}

  {% if existing_relation is none or existing_relation.is_view or should_full_refresh() %}
    {% do create_indexes(relation_for_indexes) %}
  {% endif %}

  {% if need_swap %}
      {% if existing_relation is not none and existing_relation.is_view %}
          {% do adapter.drop_relation(existing_relation) %}
      {% else %}
          {% do adapter.rename_relation(target_relation, backup_relation) %}
          {% do to_drop.append(backup_relation) %}
      {% endif %}
      {% do adapter.rename_relation(intermediate_relation, target_relation) %}
  {% endif %}

  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {% do adapter.commit() %}

  {% for rel in to_drop %}
      {% do adapter.drop_relation(rel) %}
  {% endfor %}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {{ return({'relations': [target_relation]}) }}

{%- endmaterialization %}

{% macro singlestore__validate_unique_key_columns(unique_key, dest_columns) %}
    {%- set dest_col_names = dest_columns | map(attribute='name') | list -%}

    {%- if unique_key %}
        {%- if unique_key is sequence and unique_key is not string -%}
            {%- set keys = unique_key -%}
        {%- else -%}
            {%- set keys = [unique_key] -%}
        {%- endif -%}

        {%- for k in keys %}
            {%- if k not in dest_col_names %}
                {{ exceptions.raise_compiler_error(
                    "Incremental model unique_key '" ~ k ~ "' not found in model columns: "
                    ~ dest_col_names | join(', ')
                ) }}
            {%- endif %}
        {%- endfor %}
    {%- endif %}
{% endmacro %}

{% macro singlestore__validate_merge_keys_at_runtime(target_relation, unique_key) %}
  {%- if execute -%}
    {%- set model_name = model.name if model is defined and model.name is defined else target_relation.identifier -%}
    {%- if unique_key is sequence and unique_key is not string -%}
      {%- set target_keys = unique_key | map('lower') | list | sort -%}
    {%- else -%}
      {%- set target_keys = [unique_key | lower] -%}
    {%- endif -%}
    {%- set target_keys_str = target_keys | join(',') -%}

    {%- set target_schema = target_relation.database | default(target_relation.schema, true) | trim -%}
    {%- if not target_schema -%}
      {%- set target_schema = target_relation.schema | trim -%}
    {%- endif -%}

    {% call statement('singlestore__check_merge_key', fetch_result=True) %}
      select index_name, group_concat(lower(column_name) order by lower(column_name)) as cols
      from information_schema.statistics
      where lower(table_schema) = lower('{{ escape_single_quotes(target_schema) }}')
        and lower(table_name) = lower('{{ escape_single_quotes(target_relation.identifier) }}')
        and non_unique = 0
      group by index_name
    {% endcall %}

    {%- set table = load_result('singlestore__check_merge_key').table -%}
    {%- set found = [] -%}
    {%- if table and table.rows -%}
      {%- for row in table.rows -%}
        {%- if row[1] == target_keys_str -%}
          {%- do found.append(row[0]) -%}
        {%- endif -%}
      {%- endfor -%}
    {%- endif -%}

    {%- if found | length == 0 -%}
      {{ exceptions.raise_compiler_error(
        "Incremental model '" ~ model_name ~ "' configured with incremental_strategy='merge' requires a PRIMARY KEY or UNIQUE constraint covering exactly unique_key ('" ~ target_keys | join("', '") ~ "') on target table " ~ target_relation.render() ~ ". Found no matching unique index in information_schema.statistics. Fix: add a primary_key or unique_table_key covering exactly unique_key."
      ) }}
    {%- endif -%}
  {%- endif -%}
{% endmacro %}

{% macro singlestore__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not string %}
            delete {{ adapter.quote(target.identifier) }} from {{ target }}
            join {{ source }}
            where (
                {% for key in unique_key %}
                    {{ source }}.{{ adapter.quote(key) }} = {{ target }}.{{ adapter.quote(key) }}
                    {{ "and " if not loop.last }}
                {% endfor %}
                {% if incremental_predicates %}
                    {% for predicate in incremental_predicates %}
                        and {{ predicate }}
                    {% endfor %}
                {% endif %}
            );
        {% else %}
            delete from {{ target }}
            where (
                {{ adapter.quote(unique_key) }}) in (
                select ({{ adapter.quote(unique_key) }})
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
    );
{% endmacro %}

{% macro singlestore__get_incremental_append_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = arg_dict.get("incremental_predicates", none) -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
        {%- if incremental_predicates %}
        where {{ incremental_predicates | join(' and ') }}
        {%- endif %}
    );
{% endmacro %}

{% macro singlestore__get_incremental_delete_insert_sql(arg_dict) %}
    {% if arg_dict["unique_key"] %}
        {{ singlestore__validate_unique_key_columns(arg_dict["unique_key"], arg_dict["dest_columns"]) }}
    {% endif %}
    {{ singlestore__get_delete_insert_merge_sql(arg_dict["target_relation"], arg_dict["temp_relation"], arg_dict["unique_key"], arg_dict["dest_columns"], arg_dict["incremental_predicates"]) }}
{% endmacro %}

{% macro singlestore__get_incremental_merge_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set unique_key = arg_dict["unique_key"] -%}
    {%- set incremental_predicates = arg_dict.get("incremental_predicates", none) -%}
    {%- set col_names = dest_columns | map(attribute='name') | list -%}
    {%- set model_name = model.name if model is defined and model.name is defined else target.identifier -%}

    {%- if not unique_key -%}
        {{ exceptions.raise_compiler_error(
            "Incremental model '" ~ model_name ~ "' configured with incremental_strategy='merge' requires a 'unique_key' configuration and a matching PRIMARY KEY or UNIQUE constraint on the target table. Fix: specify 'unique_key' and define a matching primary_key or unique_table_key constraint."
        ) }}
    {%- else -%}
        {{ singlestore__validate_unique_key_columns(unique_key, dest_columns) }}
        {{ singlestore__validate_merge_keys_at_runtime(target, unique_key) }}
        {%- if unique_key is sequence and unique_key is not string -%}
            {%- set unique_keys = unique_key -%}
        {%- else -%}
            {%- set unique_keys = [unique_key] -%}
        {%- endif -%}

        {%- set dest_cols_csv = get_quoted_csv(col_names) -%}

        {%- set update_assignments = [] -%}
        {%- for col in col_names -%}
            {%- if col not in unique_keys -%}
                {%- set qc = adapter.quote(col) -%}
                {%- do update_assignments.append(qc ~ ' = values(' ~ qc ~ ')') -%}
            {%- endif -%}
        {%- endfor -%}

        {%- if update_assignments | length > 0 -%}
            insert into {{ target }} ({{ dest_cols_csv }})
            select {{ dest_cols_csv }}
            from {{ source }}
            {%- if incremental_predicates %}
            where {{ incremental_predicates | join(' and ') }}
            {%- endif %}
            on duplicate key update
                {{ update_assignments | join(',\n        ') }};
        {%- else -%}
            insert ignore into {{ target }} ({{ dest_cols_csv }})
            select {{ dest_cols_csv }}
            from {{ source }}
            {%- if incremental_predicates %}
            where {{ incremental_predicates | join(' and ') }}
            {%- endif %};
        {%- endif -%}
    {%- endif -%}
{% endmacro %}

{% macro singlestore__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {{ singlestore__get_incremental_merge_sql({'target_relation': target, 'temp_relation': source, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates}) }}
{% endmacro %}

{% macro singlestore__get_incremental_default_sql(arg_dict) %}
    {% if arg_dict["unique_key"] %}
        {{ singlestore__get_incremental_delete_insert_sql(arg_dict) }}
    {% else %}
        {{ singlestore__get_incremental_append_sql(arg_dict) }}
    {% endif %}
{% endmacro %}

{% macro singlestore__get_incremental_microbatch_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}

    {%- if model.config.get('event_time') is none -%}
        {% do exceptions.raise_compiler_error("microbatch incremental strategy requires an 'event_time' model config") %}
    {%- endif -%}

    {%- if not model.batch or (not model.batch.event_time_start or not model.batch.event_time_end) -%}
        {% do exceptions.raise_compiler_error("dbt could not compute the start and end timestamps for the running batch") %}
    {%- endif -%}

    {%- set event_time = model.config.event_time -%}
    {%- set start_ts = model.batch.event_time_start.strftime('%Y-%m-%d %H:%M:%S') -%}
    {%- set end_ts = model.batch.event_time_end.strftime('%Y-%m-%d %H:%M:%S') -%}
    {%- set batch_predicate -%}
        {{ event_time }} >= '{{ start_ts }}' and {{ event_time }} < '{{ end_ts }}'
    {%- endset -%}

    {%- set predicates = [batch_predicate] -%}
    {%- for pred in incremental_predicates -%}
        {%- do predicates.append(pred) -%}
    {%- endfor -%}

    delete from {{ target }}
    where (
        {{ predicates | join(' and ') }}
    );

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
        where {{ batch_predicate }}
    );
{% endmacro %}
