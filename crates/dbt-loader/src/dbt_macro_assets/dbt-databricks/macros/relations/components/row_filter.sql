{#--
ROW FILTER MACROS
=================

Implements row-level security via Unity Catalog ROW FILTER clause.

NOTE: This file contains intentional duplication between CREATE and ALTER paths.
See https://github.com/dbt-labs/fs/blob/821247294b0c3c05d639a6e93f5bcffec0f834db/fs/sa/crates/dbt-adapter/src/relation/databricks/config/components/row_filter.rs#L142.
Keep both implementations in sync.

ARCHITECTURAL NOTE: CREATE and ALTER Paths
------------------------------------------
1. CREATE PATH (get_create_row_filter_clause):
   - Used during initial table/MV/ST creation
   - Reads raw config.get('row_filter') and validates in Jinja
   - Row filter is embedded in the CREATE statement

2. ALTER PATH (alter_set_row_filter / alter_drop_row_filter):
   - Used for subsequent changes in incremental materializations (incremental, MV, ST)
   - Receives pre-validated RowFilterConfig from Rust
   - When should_unset=True, drops the filter; when function is set, applies it

The duplication exists because:
- CREATE path reads raw config (Rust processing hasn't happened yet)
- ALTER path uses Rust-processed config via RowFilterLoader::from_local_config()
--#}

{#-- ===== FETCH MACROS ===== --#}

{%- macro fetch_row_filters(relation) -%}
  {%- if relation.is_hive_metastore() -%}
    {{ exceptions.raise_compiler_error("Row filters are not supported for Hive Metastore") }}
  {%- endif -%}
  {%- call statement('list_row_filters', fetch_result=True) -%}
    {{ fetch_row_filters_sql(relation) }}
  {%- endcall -%}
  {%- do return(load_result('list_row_filters').table) -%}
{%- endmacro -%}


{%- macro fetch_row_filters_sql(relation) -%}
  SELECT
    table_catalog,
    table_schema,
    table_name,
    filter_name,
    target_columns
  FROM `{{ relation.database }}`.`information_schema`.`row_filters`
  WHERE table_catalog = '{{ relation.database | lower }}'
    AND table_schema = '{{ relation.schema | lower }}'
    AND table_name = '{{ relation.identifier | lower }}'
{%- endmacro -%}


{#-- ===== HELPER MACROS ===== --#}

{%- macro quote_row_filter_columns(columns) -%}
  {#- Quote column names with backticks and join with comma.
      Shared by alter_set_row_filter and get_create_row_filter_clause. -#}
  {%- set quoted = [] -%}
  {%- for col in columns -%}
    {%- do quoted.append('`' ~ col ~ '`') -%}
  {%- endfor -%}
  {{- quoted | join(', ') -}}
{%- endmacro -%}


{%- macro quote_row_filter_function(function) -%}
  {#- Quote a fully qualified function name: catalog.schema.func -> `catalog`.`schema`.`func`
      Function names are stored raw internally; backticks are added at SQL generation time. -#}
  {%- set parts = function.split('.') -%}
  {%- if parts | length == 3 -%}
    `{{ parts[0] }}`.`{{ parts[1] }}`.`{{ parts[2] }}`
  {%- else -%}
    {#- Fallback for unexpected format -#}
    `{{ function }}`
  {%- endif -%}
{%- endmacro -%}


{#-- ===== ALTER MACROS (SQL-returning, don't execute) ===== --#}

{%- macro alter_set_row_filter(relation, row_filter) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }}
  {#- DIVERGENCE END -#}
  SET ROW FILTER {{ quote_row_filter_function(row_filter.function) }}
  ON ({{ quote_row_filter_columns(row_filter.columns) }})
{%- endmacro -%}


{%- macro alter_drop_row_filter(relation) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }}
  {#- DIVERGENCE END -#}
  DROP ROW FILTER
{%- endmacro -%}


{#-- ===== APPLY MACRO (executes SQL) ===== --#}

{%- macro apply_row_filter(target_relation, row_filter) -%}
  {#- Executes SQL immediately via call statement('main') -#}
  {#- row_filter is a RowFilterConfig object with is_change, should_unset, function fields -#}
  {%- if target_relation.is_hive_metastore() -%}
    {{ exceptions.raise_compiler_error("Row filters are not supported for Hive Metastore") }}
  {%- endif -%}

  {%- if row_filter.is_change -%}
    {%- if row_filter.should_unset -%}
      {%- call statement('main') -%}
        {{ alter_drop_row_filter(target_relation) }}
      {%- endcall -%}
    {%- elif row_filter.function -%}
      {%- call statement('main') -%}
        {{ alter_set_row_filter(target_relation, row_filter) }}
      {%- endcall -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}


{#-- ===== DROP IF EXISTS MACRO (for table materialization) ===== --#}

{%- macro drop_row_filter_if_exists(relation) -%}
  {#- Drops any existing row filter from a relation before CREATE OR REPLACE.
      Used by table materialization when the model has no row filter configured
      but the existing table might have one from a previous run. -#}
  {%- if not relation.is_hive_metastore() -%}
    {%- set existing_filters = fetch_row_filters(relation) -%}
    {%- if existing_filters | length > 0 -%}
      {%- call statement('drop_row_filter') -%}
        {{ alter_drop_row_filter(relation) }}
      {%- endcall -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}


{#-- ===== CREATE CLAUSE MACROS ===== --#}

{%- macro qualify_row_filter_function(function, relation) -%}
  {#- Handle 1-part and 3-part, reject 2-part. -#}
  {#- IMPORTANT: Keep in sync with Python _qualify_function_name() -#}
  {%- set parts = function.replace('`', '').split('.') -%}

  {%- if parts | length == 1 -%}
    {#- Unqualified: fn -> catalog.schema.fn -#}
    {{ relation.database }}.{{ relation.schema }}.{{ parts[0] }}
  {%- elif parts | length == 2 -%}
    {#- Ambiguous: reject -#}
    {{ exceptions.raise_compiler_error(
      "Row filter function '" ~ function ~ "' is ambiguous. " ~
      "Use either unqualified name (e.g., 'my_filter') or " ~
      "fully qualified name (e.g., 'catalog.schema.my_filter')."
    ) }}
  {%- elif parts | length == 3 -%}
    {#- Fully qualified -#}
    {{ parts[0] }}.{{ parts[1] }}.{{ parts[2] }}
  {%- else -%}
    {{ exceptions.raise_compiler_error(
      "Row filter function '" ~ function ~ "' has too many parts."
    ) }}
  {%- endif -%}
{%- endmacro -%}


{%- macro get_create_row_filter_clause(relation) -%}
  {%- set row_filter = config.get('row_filter') -%}

  {%- if not row_filter or not row_filter.get('function') -%}
    {#-- Model has no row filter - drop any existing filter from table --#}
    {{ drop_row_filter_if_exists(relation) }}
  {%- else -%}
    {#-- Model has row filter - generate the WITH ROW FILTER clause --#}
    {%- set columns = row_filter.get('columns', []) -%}

    {#- Normalize string to list -#}
    {%- if columns is string -%}
      {%- set columns = [columns] -%}
    {%- endif -%}

    {#- Validate columns is non-empty -#}
    {%- if not columns or columns | length == 0 -%}
      {{ exceptions.raise_compiler_error(
        "Row filter function '" ~ row_filter.get('function') ~ "' requires a non-empty 'columns' value."
      ) }}
    {%- endif -%}

    {%- set function = qualify_row_filter_function(row_filter.get('function'), relation) -%}
    WITH ROW FILTER {{ quote_row_filter_function(function) }} ON ({{ quote_row_filter_columns(columns) }})
  {%- endif -%}
{%- endmacro -%}


{%- macro row_filter_exists() -%}
  {%- set row_filter = config.get('row_filter') -%}
  {%- set has_function = row_filter and row_filter.get('function') -%}
  {%- set columns = row_filter.get('columns') if row_filter else none -%}

  {#- Normalize string to list -#}
  {%- if columns is string -%}
    {%- set columns = [columns] -%}
  {%- endif -%}

  {%- set has_columns = columns and columns | length > 0 -%}
  {%- do return(has_function and has_columns) -%}
{%- endmacro -%}
