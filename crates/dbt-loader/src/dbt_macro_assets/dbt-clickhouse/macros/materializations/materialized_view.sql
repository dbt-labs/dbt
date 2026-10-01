{#-
  Create or update a materialized view in ClickHouse.
  This involves creating both the materialized view itself and a
  target table that the materialized view writes to.

  External Target Mode:
  When you want the MV to write to an existing table (not auto-created), use:

    {{ materialization_target_table(ref('my_target_table')) }}

  This macro both:
  1. Registers the DAG dependency (via ref())
  2. Outputs the special comment that specifies the target table for the MV's TO clause
-#}
{%- materialization materialized_view, adapter='clickhouse' -%}

  {#- First check config, then try to extract from SQL comment -#}
  {#- Extract target table from comment. Handles formats like:
      `schema`.`table`, "schema"."table", schema.table -#}
  {%- set target_match = modules.re.search('--\\s*materialization_target_table:\\s*(.+?)\\s*$', sql, modules.re.MULTILINE) -%}
  {%- set materialization_target_table = target_match.group(1).strip() if target_match else none -%}

  {%- if materialization_target_table is not none -%}
    {%- set result = clickhouse__materialized_view_with_external_target(materialization_target_table, sql) -%}
  {%- else -%}
    {%- set result = clickhouse__materialized_view_standard(sql) -%}
  {%- endif -%}
    {{ return(result) }}
{%- endmaterialization -%}

{#-
  Split a model's sql into individual materialized view queries (if multiple exist).

  Views are delimited by `--<name>:begin` / `--<name>:end` marker comments.
  The returned mapping is keyed by marker name in the order the markers appear.
  Each key becomes a suffix on the model's relation, so marker `mv1` creates the
  view `<model>_mv1`. A model without any markers yields the single key `mv`
  covering the whole sql, yielding one `<model>_mv` view.
-#}
{% macro clickhouse__extract_mv_views(sql) %}
  {#- the name must be a single token. If the pattern permits a space in the name, an
      unrelated `--` comment becomes a part of the marker, and the name is then wrong -#}
  {% set view_names = modules.re.findall('--(?:\\s)?([^:\\s]+):begin', sql) %}

  {% set views = {} %}
  {% if view_names %}
    {% for view_name in view_names %}
      {% set view_sql = modules.re.findall('--(?:\\s)?' + view_name + ':begin(.*)--(?:\\s)?' + view_name + ':end', sql, modules.re.DOTALL)[0] %}
      {%- set _ = views.update({view_name: view_sql}) -%}
    {% endfor %}
  {% else %}
    {%- set _ = views.update({"mv": sql}) -%}
  {% endif %}

  {{ return(views) }}
{% endmacro %}

{% macro strip_identifier_quotes(ident) %}
  {%- set s = ident.strip() -%}
  {%- if (s.startswith('`') and s.endswith('`')) or
         (s.startswith('"') and s.endswith('"')) or
         (s.startswith("'") and s.endswith("'")) -%}
    {{- s[1:-1] -}}
  {%- else -%}
    {{- s -}}
  {%- endif -%}
{% endmacro %}

{% macro get_relation_from_string(relation_string) %}

  {%- set parts = relation_string.split('.') -%}

  {%- if parts | length < 2 -%}
    {{ exceptions.raise_compiler_error('Invalid relation string "' ~ relation_string ~ '". Expected format: schema.table') }}
  {%- endif -%}

  {%- set schema = strip_identifier_quotes(parts[0]) -%}
  {%- set identifier = strip_identifier_quotes(parts[1]) -%}

  {#- Get the relation from the adapter -#}
  {%- set target_relation = adapter.get_relation(database='', schema=schema, identifier=identifier) -%}

  {{ return(target_relation) }}
{% endmacro %}

{#-
  External target mode: Creates only the MV pointing to an existing table.
  The target table must exist (typically created by another dbt model).

  Usage:
    {{ materialization_target_table(ref('my_target_table')) }}
    {{ config(materialized='materialized_view') }}

  The macro does double duty:
  1. ref() registers the DAG dependency at parse time
  2. Outputs a comment that is parsed at run time for the TO clause
-#}

{% macro materialization_target_table(target_relation) %}
-- materialization_target_table: {{ target_relation }}
{% endmacro %}

{% macro clickhouse__materialized_view_with_external_target(materialization_target_table, sql) %}
  {#- The MV relation - this is what we're creating -#}
  {%- set mv_relation = this.incorporate(type='materialized_view') -%}
  {%- set cluster_clause = on_cluster_clause(mv_relation) -%}
  {%- set refreshable_clause = refreshable_mv_clause() -%}

  {#- Parse the target table string to get the actual relation -#}
  {%- set target_table_relation = get_relation_from_string(materialization_target_table) -%}
  {% if target_table_relation is none %}
    {{ exceptions.raise_compiler_error('Target table ' ~ materialization_target_table ~ ' not found in cache. It may not exist yet. Ensure the target table model runs before this MV.') }}
  {% endif %}

  {#- Check for existing MV -#}
  {%- set existing_relation = load_cached_relation(this) -%}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  {%- set view_created = True -%}

  {% if existing_relation is none %}
    {{ clickhouse__create_mv(mv_relation, materialization_target_table, cluster_clause, refreshable_clause, sql, is_main_statement=True) }};
  {% elif should_full_refresh() %}
    {{ log('Dropping existing MV ' ~ mv_relation.name ~ ' for full refresh recreation') }}
    {{ clickhouse__drop_mv(mv_relation, cluster_clause) }}
    {{ clickhouse__create_mv(mv_relation, materialization_target_table, cluster_clause, refreshable_clause, sql, is_main_statement=True) }};
  {% else %}
    {# Check if target table has changed - cannot be updated via MODIFY QUERY #}
    {% set existing_target = clickhouse__get_mv_current_target(mv_relation) %}
    {% set materialization_target_table_name = target_table_relation.schema ~ '.' ~ target_table_relation.identifier %}

    {% if existing_target != materialization_target_table_name %}
      {{ exceptions.raise_compiler_error(
        'Target table mismatch for MV "' ~ mv_relation.name ~ '": Current target is "' ~ existing_target
        ~ '", but model references "' ~ materialization_target_table_name ~ '". '
        ~ 'The TO clause cannot be changed via ALTER TABLE MODIFY QUERY. '
        ~ 'Use --full-refresh to recreate the MV with the new target table.'
      ) }}
    {% endif %}

    {{ log('Updating query of existing MV ' ~ mv_relation.name ~ ' for recreation') }}
    {{ clickhouse__modify_mv_refresh(mv_relation, existing_relation, cluster_clause) }}
    {{ clickhouse__modify_mv(mv_relation, cluster_clause, sql, is_main_statement=True) }};
    {%- set view_created = False -%}
  {% endif %}

  {% set catchup_data = config.get("catchup", True) %}
  {% if catchup_data == True and view_created == True %}
    {{ log('Executing catchup data insertion into target table ' ~ target_table_relation )}}
    {% set has_contract = config.get('contract').enforced %}
    {% do run_query(clickhouse__insert_into(target_table_relation, sql, has_contract, use_columns_from_sql=True)) %}
  {% endif %}

  {#- Cleanup and grants -#}
  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% set grant_config = config.get('grants') %}
  {% do apply_grants(mv_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(mv_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {{ adapter.commit() }}
  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {#- Return only the MV as the relation this model produces -#}
  {{ return({'relations': [mv_relation]}) }}
{% endmacro %}


{#-
  Standard mode: Creates both the target table and the MV(s).
  This is the original behavior of the materialized_view materialization.
-#}
{% macro clickhouse__materialized_view_standard(sql) %}
  {%- set target_relation = this.incorporate(type='table') -%}
  {%- set cluster_clause = on_cluster_clause(target_relation) -%}
  {%- set refreshable_clause = refreshable_mv_clause() -%}
  {%- set catchup_data = config.get('catchup', True) -%}

  {# look for an existing relation for the target table and create backup relations if necessary #}
  {%- set existing_relation = load_cached_relation(this) -%}
  {%- set backup_relation = none -%}
  {%- set preexisting_backup_relation = none -%}
  {%- set preexisting_intermediate_relation = none -%}
  {% if existing_relation is not none %}
    {%- set backup_relation_type = existing_relation.type -%}
    {%- set backup_relation = make_backup_relation(target_relation, backup_relation_type) -%}
    {%- set preexisting_backup_relation = load_cached_relation(backup_relation) -%}
    {% if not existing_relation.can_exchange %}
      {%- set intermediate_relation =  make_intermediate_relation(target_relation) -%}
      {%- set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) -%}
    {% endif %}
  {% endif %}

  {% set grant_config = config.get('grants') %}

  {{ run_hooks(pre_hooks, inside_transaction=False) }}

  -- drop the temp relations if they exist already in the database
  {{ drop_relation_if_exists(preexisting_intermediate_relation) }}
  {{ drop_relation_if_exists(preexisting_backup_relation) }}

  -- `BEGIN` happens here:
  {{ run_hooks(pre_hooks, inside_transaction=True) }}

  -- extract the sql for each of the materialized views into a map
  {% set views = clickhouse__extract_mv_views(sql) %}

  {% if backup_relation is none %}
    {{ log('Creating new materialized view ' + target_relation.name )}}
    {{ clickhouse__get_create_materialized_view_as_sql(target_relation, sql, views, catchup_data) }}
  {% elif existing_relation.can_exchange %}
    {{ log('Replacing existing materialized view ' + target_relation.name) }}
    -- in this section, we look for mvs that has the same pattern as this model, but for some reason,
    -- are not listed in the model. This might happen when using multiple mv, and renaming one of the mv in the model.
    -- In case such mv found, we raise a warning to the user, that they might need to drop the mv manually.
    {{ log('Searching for existing materialized views with the pattern of ' + target_relation.name) }}
    {{ log('Views dictionary contents: ' + views | string) }}
    {% set found_associated_mvs, expected_mv_tables = clickhouse__search_associated_mvs_to_target(existing_relation.schema, target_relation.name, views)  %}
    {% if not found_associated_mvs %}
        {{ log('No existing mvs found matching the pattern. continuing..', info=True) }}
    {% else %}
      {% for table in found_associated_mvs %}
        {% if table not in expected_mv_tables %}
          {{ log('Warning - Materialized view "' + table + '" was detected pointing to the model "' + target_relation.name + '" but was not found in this run. In case it is a renamed mv that was previously part of this model, drop it manually (!!!)', info=True) }}
        {% endif %}
      {% endfor %}
    {% endif %}
    {% if should_full_refresh() %}
      {% do clickhouse__create_target_table(backup_relation, sql, catchup_data) %}

      {# Drop MV just before exchange to minimize blind period while avoiding old MV writing to new table #}
      {{ clickhouse__drop_mvs_by_suffixes(target_relation, cluster_clause, views) }}

      {% do exchange_tables_atomic(backup_relation, existing_relation) %}

      {{ clickhouse__create_mvs(existing_relation, cluster_clause, refreshable_clause, views) }}
    {% else %}
      -- we need to have a 'main' statement
      {% call statement('main') -%}
        select 1
      {%- endcall %}

       {%- set on_schema_change = incremental_validate_on_schema_change(config.get('on_schema_change'), default='ignore') -%}
       {{ log('on_schema_change strategy for destination table of  MV: ' + on_schema_change, info=True) }}
       {%- if on_schema_change != 'ignore' -%}
        {%- set column_changes = adapter.check_incremental_schema_changes(on_schema_change, existing_relation, sql, materialization='materialized view', query_settings=config.get('query_settings', {})) -%}
        {% if column_changes %}
          {% do clickhouse__apply_column_changes(column_changes, existing_relation) %}
          {% set existing_relation = load_cached_relation(this) %}
        {% endif %}
      {%- endif %}
      -- try to alter view first to replace sql, else drop and create
      {{ clickhouse__update_mvs(target_relation, cluster_clause, refreshable_clause, views) }}

    {% endif %}
  {% else %}
    {{ log('Replacing existing materialized view ' + target_relation.name) }}
    {{ clickhouse__replace_mv(target_relation, existing_relation, intermediate_relation, backup_relation, sql, views, catchup_data) }}
  {% endif %}

  -- cleanup
  {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
  {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

  {% do persist_docs(target_relation, model) %}

  {{ run_hooks(post_hooks, inside_transaction=True) }}

  {{ adapter.commit() }}

  {{ drop_relation_if_exists(backup_relation) }}

  {{ run_hooks(post_hooks, inside_transaction=False) }}

  {% set relations = [target_relation] %}
  {% for view in views %}
    {{ relations.append(target_relation.derivative('_' + view, 'materialized_view')) }}
  {% endfor %}

  {{ return({'relations': relations}) }}
{% endmacro %}


{#
  Creates a target table for materialized views with optional catchup logic.
  If catchup is True, backfills the table with data from the SQL query.
  If catchup is False, creates an empty table without backfilling.
#}
{% macro clickhouse__create_target_table(relation, sql, catchup=True) -%}
  {% if catchup %}
    {% call statement('main') %}
      {{ get_create_table_as_sql(False, relation, sql) }}
    {% endcall %}
  {% else %}
    {{ log('Catchup config set to false, skipping table backfill for ' + relation.name) }}
    {% set has_contract = config.get('contract').enforced %}
    {{ clickhouse__create_empty_table(False, relation, sql, has_contract, statement_name='main') }}
  {% endif %}
{%- endmacro %}


{#
  There are two steps to creating a materialized view:
  1. Create a new table based on the SQL in the model
  2. Create a materialized view using the SQL in the model that inserts
  data into the table creating during step 1
#}
{% macro clickhouse__get_create_materialized_view_as_sql(relation, sql, views, catchup=True ) -%}
  {% do clickhouse__create_target_table(relation, sql, catchup) %}
  {%- set cluster_clause = on_cluster_clause(relation) -%}
  {%- set refreshable_clause = refreshable_mv_clause() -%}
  {%- set mv_relation = relation.derivative('_mv', 'materialized_view') -%}
  {{ clickhouse__create_mvs(relation, cluster_clause, refreshable_clause, views) }}
{%- endmacro %}

{% macro clickhouse__drop_mv(mv_relation, cluster_clause)  -%}
  {% call statement('drop existing mv: ' + mv_relation.name) -%}
    drop view if exists {{ mv_relation }} {{ cluster_clause }}
  {% endcall %}
{%- endmacro %}

{% macro clickhouse__create_mv(mv_relation, target_relation, cluster_clause, refreshable_clause, view_sql, is_main_statement=False)  -%}
  {% set statement_name = 'main' if is_main_statement else 'create existing mv: ' + mv_relation.name -%}
  {% call statement(statement_name) -%}
    create materialized view if not exists {{ mv_relation }} {{ cluster_clause }}
    {{ refreshable_clause }}
    to {{ target_relation }}
    as {{ view_sql }}
  {% endcall %}
{%- endmacro %}

{% macro clickhouse__modify_mv(mv_relation, cluster_clause, view_sql, is_main_statement=False)  -%}
  {% set statement_name = 'main' if is_main_statement else 'modify existing mv: ' + mv_relation.name -%}
  {% call statement(statement_name) -%}
    alter table {{ mv_relation }} {{ cluster_clause }} modify query {{ view_sql }}
  {% endcall %}
{%- endmacro %}

{% macro clickhouse__get_mv_current_target(mv_relation) %}
  {% set query %}
    {# 
      '\x3F' is the ClickHouse string-literal escape for '?' (regex quantifier); some drivers treat a literal '?' in query text as a bind parameter.
      Literal can be configured back to '?' after https://github.com/ClickHouse/adbc_clickhouse/issues/53 is fixed.
    #}
    select replaceRegexpOne(create_table_query, '.*TO\\s+`\x3F([^`\\s(]+)`\x3F\\.`\x3F([^`\\s(]+)`\x3F.*', '\\1.\\2') as target_table
    from system.tables
    where database = '{{ mv_relation.schema }}'
      and name = '{{ mv_relation.identifier }}'
      and engine = 'MaterializedView'
  {% endset %}
  {% set result = run_query(query) %}
  {% if result and result.columns and result.columns[0].values() | length > 0 %}
    {{ return(result.columns[0].values()[0]) }}
  {% endif %}
  {{ return(none) }}
{% endmacro %}

{#-
  Applies refresh parameter changes to an existing refreshable MV in place via
  ALTER TABLE ... MODIFY REFRESH (https://github.com/ClickHouse/dbt-clickhouse/issues/611).
  Like MODIFY QUERY, the statement is deliberately issued on every run without diffing
  against the deployed schedule (simplicity over avoiding no-op DDL): it replaces the
  whole schedule (interval, randomization and dependencies), so reissuing unchanged
  parameters is a harmless no-op.
  The deployed state comes from the cached relation (is_refreshable / refreshable_append),
  so no extra round trip to ClickHouse is needed. Transitions that ClickHouse cannot apply
  in place (regular <-> refreshable, APPEND <-> non-APPEND) raise an error pointing the
  user to --full-refresh instead of being silently ignored.
  mv_relation and existing_relation always refer to the same MV: both call sites resolve
  existing_relation from mv_relation's own schema and identifier. The split exists because
  mv_relation is the render target while existing_relation carries the cache-populated flags.
-#}
{% macro clickhouse__modify_mv_refresh(mv_relation, existing_relation, cluster_clause) %}
  {%- set refreshable_config = config.get('refreshable') -%}
  {%- if refreshable_config is none or refreshable_config == false -%}
    {%- if existing_relation.is_refreshable -%}
      {% do exceptions.raise_compiler_error(
        'Materialized view "' ~ mv_relation.name ~ '" is refreshable, but the model does not define a "refreshable" config. '
        ~ 'A refreshable MV cannot be converted to a regular MV in place. '
        ~ 'If you want to change this MV to a regular MV, please run with --full-refresh.'
      ) %}
    {%- endif -%}
  {%- else -%}
    {%- if not existing_relation.is_refreshable -%}
      {% do exceptions.raise_compiler_error(
        'Materialized view "' ~ mv_relation.name ~ '" is not refreshable, but the model defines a "refreshable" config. '
        ~ 'A regular MV cannot be converted to a refreshable MV in place. '
        ~ 'If you want to change this MV to be refreshable, please run with --full-refresh.'
      ) %}
    {%- endif -%}
    {%- set config_append = true if (refreshable_config is mapping and refreshable_config.get('append', false)) else false -%}
    {%- if config_append != existing_relation.refreshable_append -%}
      {% do exceptions.raise_compiler_error(
        'The APPEND setting of materialized view "' ~ mv_relation.name ~ '" does not match the model config. '
        ~ 'APPEND cannot be changed via ALTER TABLE ... MODIFY REFRESH. '
        ~ 'If you want to change it, please run with --full-refresh.'
      ) %}
    {%- endif -%}
    {{ log('Updating refresh parameters of existing MV ' ~ mv_relation.name) }}
    {#- ClickHouse requires the APPEND keyword in MODIFY REFRESH to match the view's
        creation-time APPEND mode ("Adding or removing APPEND is not supported"), so the
        full clause is used here; the guard above guarantees config and deployed agree. -#}
    {%- set modify_refresh_clause = refreshable_mv_clause() -%}
    {% call statement('modify refresh of existing mv: ' ~ mv_relation.name) -%}
      alter table {{ mv_relation }} {{ cluster_clause }} modify {{ modify_refresh_clause }}
    {%- endcall %}
  {%- endif -%}
{% endmacro %}

{% macro clickhouse__update_mv(mv_relation, target_relation, cluster_clause, refreshable_clause, view_sql)  -%}
  {% set existing_relation = adapter.get_relation(database=mv_relation.database, schema=mv_relation.schema, identifier=mv_relation.identifier) %}
  {% if existing_relation %}
    {{ clickhouse__modify_mv_refresh(mv_relation, existing_relation, cluster_clause) }}
    {{ clickhouse__modify_mv(mv_relation, cluster_clause, view_sql) }};
  {% else %}
    {{ clickhouse__drop_mv(mv_relation, cluster_clause) }};
    {{ clickhouse__create_mv(mv_relation, target_relation, cluster_clause, refreshable_clause, view_sql) }};
  {% endif %}

{%- endmacro %}

{% macro clickhouse__drop_mvs_by_suffixes(target_relation, cluster_clause, views_suffixes)  -%}
  {% for suffix in views_suffixes.keys() %}
    {%- set mv_relation = target_relation.derivative('_' + suffix, 'materialized_view') -%}
    {{ clickhouse__drop_mv(mv_relation, cluster_clause) }};
  {% endfor %}
{%- endmacro %}

{% macro clickhouse__drop_mvs_by_names(target_relation, cluster_clause, mvs_names)  -%}
  {% for mvs_name in mvs_names %}
    {%- set mv_relation = target_relation.derivative(mvs_name, 'materialized_view', interpret_suffix_as_full_identifier=True) -%}
    {{ clickhouse__drop_mv(mv_relation, cluster_clause) }};
  {% endfor %}
{%- endmacro %}

{% macro clickhouse__create_mvs(target_relation, cluster_clause, refreshable_clause, views)  -%}
  {% for view, view_sql in views.items() %}
    {%- set mv_relation = target_relation.derivative('_' + view, 'materialized_view') -%}
    {{ clickhouse__create_mv(mv_relation, target_relation, cluster_clause, refreshable_clause, view_sql) }};
  {% endfor %}
{%- endmacro %}

{% macro clickhouse__search_associated_mvs_to_target(relation_schema, relation_name, mv_suffixes)  -%}
  {% set tables_query %}
    select name
    from system.tables
    where engine = 'MaterializedView'
      and extract(create_table_query, 'TO\\s+([^\\s(]+)') = '{{ relation_schema }}.{{ relation_name }}'
  {% endset %}

  {% set expected_mvs = [] %}
  {% for suffix in mv_suffixes.keys() %}
    {% do expected_mvs.append(relation_name ~ "_" ~ suffix) %}
  {% endfor %}
  {{ log('Model mvs to replace ' + expected_mvs | string) }}

  {% set mvs_found = run_query(tables_query) %}
  {% if mvs_found is not none and mvs_found.columns %}
    {% set mv_found_names = mvs_found.columns[0].values() %}
    {{ log('Current mvs found in ClickHouse are: ' + mv_found_names | join(', ')) }}
    {{ return((mv_found_names, expected_mvs,)) }}
  {% else %}
    {{ return(([], expected_mvs,)) }}
  {% endif %}
{%- endmacro %}


{% macro clickhouse__drop_associated_mv_if_it_was_automatically_created(target_relation)  -%}
  {#-
    Limitations of this logic:
     - Only covers situations where 1 mv was created. Don't cover multiple mvs with different names.
     - Only checks current relation's database.
    We print logs in case we find other mvs in that database
  -#}
  {% set views = {'mv': ''} %}
  {% set found_associated_mvs, expected_mv_tables = clickhouse__search_associated_mvs_to_target(target_relation.schema, target_relation.name, views) %}
  {% if found_associated_mvs is not none %}
    {% for table in found_associated_mvs %}
      {% if table not in expected_mv_tables %}
        {{ log('Warning - Materialized View "' + table + '" was detected pointing to the model name "' + target_relation.name + '" that was just updated/removed. It can\'t be automatically removed by DBT. Drop it manually if needed (!!!)', info=True) }}
      {% endif %}
    {% endfor %}
  {% endif %}
  {%- set cluster_clause = on_cluster_clause(target_relation) -%}
  {% set matching_mvs = [] %}
  {% for mv in found_associated_mvs %}
    {% if mv in expected_mv_tables %}
      {% do matching_mvs.append(mv) %}
    {% endif %}
  {% endfor %}
  {{ clickhouse__drop_mvs_by_names(target_relation, cluster_clause, matching_mvs) }}
{%- endmacro %}

{% macro clickhouse__update_mvs(target_relation, cluster_clause, refreshable_clause, views)  -%}
  {% for view, view_sql in views.items() %}
    {%- set mv_relation = target_relation.derivative('_' + view, 'materialized_view') -%}
    {{ clickhouse__update_mv(mv_relation, target_relation, cluster_clause, refreshable_clause, view_sql) }};
  {% endfor %}
{%- endmacro %}

{% macro clickhouse__replace_mv(target_relation, existing_relation, intermediate_relation, backup_relation, sql, views, catchup=True) %}
  {# drop existing materialized view while we recreate the target table #}
  {%- set cluster_clause = on_cluster_clause(target_relation) -%}
  {%- set refreshable_clause = refreshable_mv_clause() -%}
  {{ clickhouse__drop_mvs_by_suffixes(target_relation, cluster_clause, views) }}

  {# recreate the target table #}
  {% do clickhouse__create_target_table(intermediate_relation, sql, catchup) %}
  {{ adapter.rename_relation(existing_relation, backup_relation) }}
  {{ adapter.rename_relation(intermediate_relation, target_relation) }}

  {# now that the target table is recreated, we can finally create our new view #}
  {{ clickhouse__create_mvs(target_relation, cluster_clause, refreshable_clause, views) }}
{% endmacro %}

{#-
  Renders the refresh clause of a refreshable MV
  (REFRESH ... [RANDOMIZE FOR ...] [DEPENDS ON ...] [APPEND]).
-#}
{% macro refreshable_mv_clause() %}
  {%- if config.get('refreshable') is not none and config.get('refreshable') != false -%}

    {% set refreshable_config = config.get('refreshable') %}
    {% if refreshable_config is not mapping %}
      {% do exceptions.raise_compiler_error(
        "The 'refreshable' configuration must be defined as a dictionary. Please review the docs for more details."
      ) %}
    {% endif %}

    {% set refresh_interval = refreshable_config.get('interval', none) %}
    {% set refresh_randomize = refreshable_config.get('randomize', none) %}
    {% set depends_on = refreshable_config.get('depends_on', none) %}
    {% set depends_on_validation = refreshable_config.get('depends_on_validation', false) %}
    {% set append = refreshable_config.get('append', false) %}

    {% if not refresh_interval %}
      {% do exceptions.raise_compiler_error(
        "The 'refreshable' configuration is defined, but 'interval' is missing. "
        ~ "This is required to create a refreshable materialized view."
      ) %}
    {% endif %}

    {% if refresh_interval %}
      REFRESH {{ refresh_interval }}
      {# This is a comment to force a new line between REFRESH and RANDOMIZE clauses #}
      {%- if refresh_randomize -%}
        RANDOMIZE FOR {{ refresh_randomize }}
      {%- endif -%}
    {% endif %}

    {% if depends_on %}
      {% set depends_on_list = [] %}

      {% if depends_on is string %}
        {% set depends_on_list = [depends_on] %}
      {% elif depends_on is iterable %}
        {% set temp_list = depends_on_list %}
        {%- for dep in depends_on %}
          {% if dep is string %}
            {% do temp_list.append(dep) %}
          {% else %}
            {% do exceptions.raise_compiler_error(
              "The 'depends_on' configuration must be either a string or a list of strings."
            ) %}
          {% endif %}
        {% endfor %}
        {% set depends_on_list = temp_list %}
      {% else %}
        {% do exceptions.raise_compiler_error(
          "The 'depends_on' configuration must be either a string or a list of strings."
        ) %}
      {% endif %}

      {% if depends_on_validation and depends_on_list | length > 0 %}
        {%- for dep in depends_on_list %}
          {% do validate_refreshable_mv_existence(dep) %}
        {%- endfor %}
      {% endif %}

      DEPENDS ON {{ depends_on_list | join(', ') }}
    {% endif %}

    {%- if append -%}
      APPEND
    {%- endif -%}

  {%- endif -%}
{% endmacro %}


{% macro validate_refreshable_mv_existence(mv) %}
  {{ log(mv + ' was recognized as a refreshable mv dependency, checking its existence') }}
  {% set default_database = "default" %}

  {%- set parts = mv.split('.') %}
  {%- if parts | length == 2 %}
    {%- set database = parts[0] %}
    {%- set table = parts[1] %}
  {%- else %}
    {%- set database = default_database %}
    {%- set table = parts[0] %}
  {%- endif %}

  {%- set condition = "database='" + database + "' and view='" + table + "'" %}

  {% set query %}
    select count(*)
    from system.view_refreshes
    where {{ condition }}
  {% endset %}

  {% set tables_result = run_query(query) %}
    {{ log(tables_result.columns[0].values()[0]) }}
  {% if tables_result.columns[0].values()[0] > 0 %}
    {{ log('MV ' + mv + ' exists.') }}
  {% else %}
    {% do exceptions.raise_compiler_error(
      'No existing MV found matching MV: ' + mv
    ) %}
  {% endif %}
{% endmacro %}
