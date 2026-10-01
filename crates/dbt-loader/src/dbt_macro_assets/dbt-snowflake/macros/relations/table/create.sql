{% macro snowflake__create_table_as(temporary, relation, compiled_code, language='sql') -%}

    {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

    {%- if language == 'sql' -%}
        {%- if temporary -%}
            {{ snowflake__create_table_temporary_sql(relation, compiled_code) }}
        {%- elif catalog_relation.catalog_type == 'INFO_SCHEMA' -%}
            {{ snowflake__create_table_info_schema_sql(relation, compiled_code) }}
        {%- elif catalog_relation.catalog_type == 'BUILT_IN' -%}
            {{ snowflake__create_table_built_in_sql(relation, compiled_code) }}
        {# DIVERGENCE #}
        {%- elif catalog_relation.catalog_type == 'ICEBERG_REST' -%}
            {{ snowflake__create_table_iceberg_rest_sql(relation, compiled_code) }}
        {%- else -%}
            {% do exceptions.raise_compiler_error('Unexpected model config for: ' ~ relation) %}
        {%- endif -%}

    {%- elif language == 'python' -%}
        {%- if catalog_relation.catalog_type == 'BUILT_IN' %}
            {% do exceptions.raise_compiler_error('Iceberg is incompatible with Python models. Please use a SQL model for the iceberg format.') %}
        {%- else -%}
            {{ py_write_table(compiled_code, relation, temporary) }}
        {%- endif %}

    {%- else -%}
        {% do exceptions.raise_compiler_error("snowflake__create_table_as macro didn't get supported language, it got %s" % language) %}

    {%- endif -%}

{% endmacro %}


{% macro snowflake__create_table_transient_sql(relation, compiled_code) -%}
{#-
    Implements CREATE TRANSIENT TABLE ... AS SELECT for use as an incremental
    tmp relation. Unlike session-scoped temporary tables, transient tables
    persist in the catalog (enabling Snowflake lineage tracking) but have no
    fail-safe period, avoiding the storage costs of permanent tables.
    https://docs.snowflake.com/en/sql-reference/sql/create-table
-#}

{%- set contract_config = config.get('contract') -%}
{%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(compiled_code) -}}
    {%- set compiled_code = get_select_subquery(compiled_code) -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header is not none }}

create or replace transient table {{ relation }}
    {%- if contract_config.enforced %}
    {{ get_table_columns_and_constraints() }}
    {%- endif %}
as (
    {{ compiled_code }}
    )
;

{%- endmacro %}


{% macro snowflake__create_table_temporary_sql(relation, compiled_code) -%}
{#-
    Implements CREATE TEMPORARY TABLE and CREATE TEMPORARY TABLE ... AS SELECT:
    https://docs.snowflake.com/en/sql-reference/sql/create-table
    https://docs.snowflake.com/en/sql-reference/sql/create-table#create-table-as-select-also-referred-to-as-ctas
-#}

{%- set contract_config = config.get('contract') -%}
{%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(compiled_code) -}}
    {%- set compiled_code = get_select_subquery(compiled_code) -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header is not none }}

create or replace temporary table {{ relation }}
    {%- if contract_config.enforced %}
    {{ get_table_columns_and_constraints() }}
    {%- endif %}
as (
    {{ compiled_code }}
    )
;

{%- endmacro %}


{% macro snowflake__create_table_info_schema_sql(relation, compiled_code) -%}
{#-
    Implements CREATE TABLE and CREATE TABLE ... AS SELECT:
    https://docs.snowflake.com/en/sql-reference/sql/create-table
    https://docs.snowflake.com/en/sql-reference/sql/create-table#create-table-as-select-also-referred-to-as-ctas
-#}

{%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

{%- if catalog_relation.is_transient -%}
    {%- set transient='transient ' -%}
{%- else -%}
    {%- set transient='' -%}
{%- endif -%}

{# DIVERGENCE BEGIN #}
{# -- begin TODO: store all this under the CatalogRelation type for core compliance of
   -- catalog relation based ddl; determine why 'is not none' is different in Fusion #}
{%- set enable_automatic_clustering = config.get('automatic_clustering', default=false) -%}
{%- set cluster_by_keys = config.get('cluster_by', default=none) -%}

{%- if cluster_by_keys and cluster_by_keys is string -%}
  {%- set cluster_by_keys = [cluster_by_keys] -%}
{%- endif -%}

{%- if cluster_by_keys -%}
  {%- set cluster_by_string = cluster_by_keys|join(", ")-%}
{% else %}
  {%- set cluster_by_string = none -%}
{%- endif -%}
{# -- end TODO #}
{# DIVERGENCE END #}

{%- set copy_grants = config.get('copy_grants', default=false) -%}
{%- set copy_tags = config.get('copy_tags', default=false) -%}
{%- set row_access_policy = config.get('row_access_policy', default=none) -%}
{%- set table_tag = config.get('table_tag', default=none) -%}

{%- set contract_config = config.get('contract') -%}
{%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(compiled_code) -}}
    {%- set compiled_code = get_select_subquery(compiled_code) -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header is not none }}

create or replace {{ transient }} table {{ relation }}
    {%- set contract_config = config.get('contract') -%}
    {%- if contract_config.enforced %}
    {{ get_table_columns_and_constraints() }}
    {%- endif %}
    {% if copy_grants -%} copy grants {%- endif %}
    {% if copy_tags -%} copy tags {%- endif %}
    {% if row_access_policy -%} with row access policy {{ row_access_policy }} {%- endif %}
    {% if table_tag -%} with tag ({{ table_tag }}) {%- endif %}
    as (
    {#- DIVERGENCE: when we store this under the Catalog Relation, we can change this back to how it is in Core -#}
	{%- if cluster_by_string -%}
        select * from (
            {{ compiled_code }}
        )
        order by (
            {{ cluster_by_string }} {# DIVERGENCE #}
        )
        {%- else -%}
        {{ compiled_code }}
        {%- endif %}
    )
;

{# DIVERGENCE #}
{% if cluster_by_string -%}
alter table {{relation}} cluster by ({{cluster_by_string}});
{%- endif -%}

{# DIVERGENCE: unreachable — TBD whether this belongs here once cluster_by moves onto CatalogRelation #}
{%- if false -%}
alter table {{ relation }} cluster by ({{ catalog_relation.cluster_by }});
{%- endif -%}

{% if enable_automatic_clustering and cluster_by_string %} {# DIVERGENCE #}
alter table {{ relation }} resume recluster;
{%- endif -%}

{%- endmacro %}


{% macro snowflake__create_table_built_in_sql(relation, compiled_code) -%}
{#-
    Implements CREATE ICEBERG TABLE and CREATE ICEBERG TABLE ... AS SELECT (Snowflake as the Iceberg catalog):
    https://docs.snowflake.com/en/sql-reference/sql/create-iceberg-table-snowflake

    Limitations:
    - Iceberg does not support temporary tables (use a standard Snowflake table)
-#}

{%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}


{# DIVERGENCE BEGIN #}
{# -- begin TODO: store all this under the CatalogRelation type for core compliance of
   -- catalog relation based ddl; determine why 'is not none' is different in Fusion #}
{%- set enable_automatic_clustering = config.get('automatic_clustering', default=false) -%}
{%- set cluster_by_keys = config.get('cluster_by', default=none) -%}

{%- if cluster_by_keys and cluster_by_keys is string -%}
  {%- set cluster_by_keys = [cluster_by_keys] -%}
{%- endif -%}

{%- if cluster_by_keys -%}
  {%- set cluster_by_string = cluster_by_keys|join(", ")-%}
{% else %}
  {%- set cluster_by_string = none -%}
{%- endif -%}
{# -- end TODO #}
{# DIVERGENCE END #}

{%- set partition_by_keys = get_partition_by_keys(config) -%} {# DIVERGENCE: upstream passes catalog_relation; pending partition_by on CatalogRelation #}
{%- if partition_by_keys -%}
  {%- set partition_by_string = partition_by_keys|join(", ")-%}
{% else %}
  {%- set partition_by_string = none -%}
{%- endif -%}

{%- set copy_grants = config.get('copy_grants', default=false) -%}

{%- set row_access_policy = config.get('row_access_policy', default=none) -%}
{%- set table_tag = config.get('table_tag', default=none) -%}

{%- set contract_config = config.get('contract') -%}
{%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(compiled_code) -}}
    {%- set compiled_code = get_select_subquery(compiled_code) -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header is not none }}

create or replace iceberg table {{ relation }}
    {%- if contract_config.enforced %}
    {{ get_table_columns_and_constraints() }}
    {%- endif %}
    {{ optional('external_volume', catalog_relation.external_volume, "'") }}
    catalog = 'SNOWFLAKE'  -- required, and always SNOWFLAKE for built-in Iceberg tables
    {{ optional('base_location', catalog_relation.base_location, "'") }}
    {% if partition_by_string -%} partition by ({{ partition_by_string }}) {%- endif %}
    {{ optional('storage_serialization_policy', catalog_relation.storage_serialization_policy, "'")}}
    {{ optional('max_data_extension_time_in_days', catalog_relation.max_data_extension_time_in_days)}}
    {{ optional('data_retention_time_in_days', catalog_relation.data_retention_time_in_days)}}
    {{ optional('change_tracking', catalog_relation.change_tracking)}}
    {{ optional('iceberg_version', catalog_relation.iceberg_version)}}
    {% if copy_grants -%} copy grants {%- endif %}
    {% if row_access_policy -%} with row access policy {{ row_access_policy }} {%- endif %}
    {% if table_tag -%} with tag ({{ table_tag }}) {%- endif %}
as (
    {%- if cluster_by_string -%} {# DIVERGENCE #}
    select * from (
	{{ compiled_code }}
    ) order by ({{ cluster_by_string }}) {# DIVERGENCE #}
    {%- else -%}
    {{ compiled_code }}
    {%- endif %}
    )
;

{# DIVERGENCE #}
{% if cluster_by_string -%}
alter iceberg table {{relation}} cluster by ({{cluster_by_string}});
{%- endif -%}

{% if enable_automatic_clustering and cluster_by_string %} {# DIVERGENCE #}
alter iceberg table {{ relation }} resume recluster;
{%- endif -%}

{%- endmacro %}


{% macro snowflake__create_table_iceberg_rest_with_glue(relation, compiled_code, catalog_relation) -%}
{#-
    Creates an Iceberg table for Catalog Linked Databases (e.g., AWS Glue) with explicit column definitions.
    This is used when CTAS is not supported.

    This macro is specifically for CLD where we need to create the table with an explicit schema
    because CTAS is not available.
-#}

{# Step 0: Create a Glue-compatible relation (lowercase + double-quoted) #}
{# DIVERGENCE: FIXME: see the comments from snowflake__create_table_iceberg_rest_sql above #}
{% set glue_relation = make_glue_compatible_relation(relation) %}

{# Step 1: Get the schema from the compiled query #}
{% set sql_columns = get_column_schema_from_query(compiled_code) %}

{# Step 2: Create the iceberg table in the CLD with explicit column definitions #}

{%- set row_access_policy = config.get('row_access_policy', default=none) -%}
{%- set table_tag = config.get('table_tag', default=none) -%}

{%- set partition_by_keys = get_partition_by_keys(config) -%} {# DIVERGENCE: upstream passes catalog_relation; pending partition_by on CatalogRelation #}
{%- if partition_by_keys -%}
  {# HACK: Force columns to be lowercase and quoted in glue #}
  {%- set partition_by_keys_quotes = [] -%}
  {%- for key in partition_by_keys -%}
    {% set quoted_key = '"' ~ key.lower() ~ '"' %}
    {%- do partition_by_keys_quotes.append(quoted_key) -%}
  {%- endfor -%}
  {%- set partition_by_string = partition_by_keys_quotes | join(", ")-%}
{% else %}
  {%- set partition_by_string = none -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header is not none }}

{# Step 2a: Check if relation exists and drop if necessary (CLD doesn't support CREATE OR REPLACE) #}
{% set existing_relation = adapter.get_relation(database=glue_relation.database, schema=glue_relation.schema, identifier=glue_relation.identifier) %}
{% if existing_relation %}
    drop table if exists {{ existing_relation }};
{% endif %}

{# Step 2b: Create the table with explicit column definitions #}
create iceberg table {{ glue_relation }} (
    {%- for column in sql_columns -%}
        {% if column.data_type == "FIXED" %}
            {%- set data_type = "INT" -%}
        {% elif "character varying" in column.data_type %}
            {%- set data_type = "STRING" -%}
        {% elif "timestamp" in column.data_type %}
            {%- set data_type = "TIMESTAMP" -%}
        {% else %}
            {%- set data_type = column.data_type -%}
        {% endif %}
        {{ adapter.quote(column.name.lower()) }} {{ data_type }}
        {%- if not loop.last %}, {% endif -%}
    {% endfor -%}
)
{% if partition_by_string -%} partition by ({{ partition_by_string }}) {%- endif %}
{{ optional('external_volume', catalog_relation.external_volume, "'") }}
{{ optional('iceberg_version', catalog_relation.iceberg_version)}}
{{ optional('target_file_size', catalog_relation.target_file_size, "'") }}
{{ optional('auto_refresh', catalog_relation.auto_refresh) }}
{{ optional('max_data_extension_time_in_days', catalog_relation.max_data_extension_time_in_days)}}
{#
    TODO: COPY GRANTS is in the CLD grammar but this macro uses DROP + CREATE (not CREATE OR REPLACE),
    so there is no source object to copy from after the drop. Once CREATE OR REPLACE is proven stable
    for Glue CLD and adopted here, re-enable: {% if copy_grants -%} copy grants {%- endif %}
    and add a copy_grants model for the Glue CLD path in adapters_snowflake_iceberg_grants_tags.
#}
{% if row_access_policy -%} with row access policy {{ row_access_policy }} {%- endif %}
{% if table_tag -%} with tag ({{ table_tag }}) {%- endif %}
;

{# Step 3: Insert data from the view (in regular DB) into the table (in CLD) #}
insert into {{ glue_relation }}
    {{ compiled_code }};

{%- endmacro %}


{# DIVERGENCE: FIXME:
    From @ajhlee-dbt:
    this and the divergence for `{% set glue_relation = make_glue_compatible_relation(relation) %}` are quoting issues for catalog-linked databases.
    Fusion has an entirely separate path for just Glue CLD because the hack in Core to override quoting does not work
#}
{% macro snowflake__create_table_iceberg_rest_sql(relation, compiled_code) -%}
{#-
    Implements CREATE ICEBERG TABLE ... CATALOG('catalog_name') (external REST catalog):
    https://docs.snowflake.com/en/sql-reference/sql/create-iceberg-table-rest

    Limitations:
    - Iceberg does not support temporary tables (use a standard Snowflake table)
    - Iceberg REST does not support CREATE OR REPLACE
    - Iceberg catalogs do not support table renaming operations
    - For existing tables, we must DROP the table first before creating the new one
-#}

{# DIVERGENCE BEGIN: v1/v2 behavior shim; upstream uses catalog_relation.linked_catalog_provider.is_glue directly; remove once use_catalogs_v2 is on by default.
   `adapter.behavior.use_catalogs_v2` is a Fusion-only behavior flag; accessing it under
   dbt-core raises a CompilationError that `is defined` does not catch. Fusion is dbt 2.x
   and dbt-core is 1.x, so gate the access on `dbt_version.startswith('2.')`.
   See dbt-labs/fs#10659. #}
{% macro is_glue_catalog_linked_database(catalog_relation) -%}
  {% if dbt_version.startswith('2.') and adapter.behavior.use_catalogs_v2.no_warn %}
    {{ return(catalog_relation.linked_catalog_provider.is_glue) }}
  {% elif catalog_relation.catalog_linked_database_type is defined %}
    {# -- v1 fallback: use the legacy catalog_linked_database_type surface -- #}
    {{ return(catalog_relation.catalog_linked_database_type | lower == 'glue') }}
  {% else %}
    {% do exceptions.raise_compiler_error('unreachable: catalog linked database provider must be derivable for this branch') %}
  {% endif %}
{%- endmacro %}
{# DIVERGENCE END #}

{%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

{%- set row_access_policy = config.get('row_access_policy', default=none) -%}
{%- set table_tag = config.get('table_tag', default=none) -%}

{%- set partition_by_keys = get_partition_by_keys(config) -%} {# DIVERGENCE: upstream passes catalog_relation; pending partition_by on CatalogRelation #}
{%- if partition_by_keys -%}
  {%- set partition_by_string = partition_by_keys|join(", ")-%}
{% else %}
  {%- set partition_by_string = none -%}
{%- endif -%}

{%- set contract_config = config.get('contract') -%}
{%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(compiled_code) -}}
    {%- set compiled_code = get_select_subquery(compiled_code) -%}
{%- endif -%}

{%- set sql_header = config.get('sql_header', none) -%}
{{ sql_header if sql_header }}

{# Check if this is a Glue catalog-linked database - Glue doesn't support CTAS #}
{%- set is_glue_cld = is_glue_catalog_linked_database(catalog_relation) -%}

{%- if is_glue_cld -%}
    {# Delegate to Glue-specific macro (handles its own drop logic) #}
    {{ snowflake__create_table_iceberg_rest_with_glue(relation, compiled_code, catalog_relation) }}

{%- else -%}
    {# Standard Iceberg REST catalog - supports CTAS #}
    
    {# Check if relation exists and drop if necessary #}
    {% set existing_relation = adapter.get_relation(database=relation.database, schema=relation.schema, identifier=relation.identifier) %}
    {% if existing_relation %}
        {# Iceberg catalogs don't support table renaming, so we must drop first #}
        drop table if exists {{ existing_relation }};
    {% endif %}
    create iceberg table {{ relation }}
        {%- if contract_config.enforced %}
        {{ get_table_columns_and_constraints() }}
        {%- endif %}
        {# DIVERGENCE BEGIN: `adapter.behavior.use_catalogs_v2` is a Fusion-only behavior flag.
           Accessing it under dbt-core (e.g. the v2-parser handoff) raises a CompilationError
           that `is defined` does not catch. Fusion is dbt 2.x and dbt-core is 1.x, so gate
           the access on `dbt_version.startswith('2.')`. See dbt-labs/fs#10659. #}
        {%- if not (
            (dbt_version.startswith('2.') and adapter.behavior.use_catalogs_v2.no_warn and catalog_relation|attr('catalog_database'))
            or catalog_relation|attr('catalog_linked_database')
        ) -%}
        {# DIVERGENCE END #}
        {{ optional('external_volume', catalog_relation.external_volume, "'") }}
        catalog = '{{ catalog_relation.catalog_name }}'  -- external REST catalog name
        {{ optional('base_location', catalog_relation.base_location, "'") }}
        {%- endif %}
        {% if partition_by_string -%} partition by ({{ partition_by_string }}) {%- endif %}
        {{ optional('iceberg_version', catalog_relation.iceberg_version)}}
        {{ optional('target_file_size', catalog_relation.target_file_size, "'") }}
        {{ optional('auto_refresh', catalog_relation.auto_refresh) }}
        {{ optional('max_data_extension_time_in_days', catalog_relation.max_data_extension_time_in_days)}}
        {#
            TODO: COPY GRANTS is in the CLD grammar but this macro uses DROP + CREATE (not CREATE OR
            REPLACE), so there is no source object to copy from after the drop. Once CREATE OR REPLACE
            is proven stable for REST CLD and adopted here, re-enable:
            {% if copy_grants -%} copy grants {%- endif %}
            and add a copy_grants model for the REST CLD path in adapters_snowflake_iceberg_grants_tags.
            See: https://docs.snowflake.com/en/sql-reference/sql/create-iceberg-table#create-iceberg-table-as-select-also-referred-to-as-ctas
        #}
        {% if row_access_policy -%} with row access policy {{ row_access_policy }} {%- endif %}
        {% if table_tag -%} with tag ({{ table_tag }}) {%- endif %}
    as (
        {{ compiled_code }}
    );
{%- endif -%}

{%- endmacro %}


{% macro py_write_table(compiled_code, target_relation, temporary=False) %}

{%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

{% if temporary %}
    {%- set table_type='temporary' -%}
{% elif catalog_relation.is_transient %}
    {%- set table_type='transient' -%}
{% endif %}

{{ compiled_code }}


def materialize(session, df, target_relation):
    # make sure pandas exists
    import importlib.util
    package_name = 'pandas'
    if importlib.util.find_spec(package_name):
        import pandas
        if isinstance(df, pandas.core.frame.DataFrame):
            session.use_database(target_relation.database)
            session.use_schema(target_relation.schema)
            # session.write_pandas does not have overwrite function
            df = session.createDataFrame(df)
    {% set target_relation_name = resolve_model_name(target_relation) %}
    df.write.mode("overwrite").save_as_table('{{ target_relation_name }}', table_type='{{table_type}}')


def main(session):
    dbt = dbtObj(session.table)
    df = model(dbt, session)
    materialize(session, df, dbt.this)
    return "OK"

{% endmacro %}

{# DIVERGENCE #}
{# -- begin TODO: store all this under the CatalogRelation type for core compliance of
   -- catalog relation based ddl, then pass in the catalog relation here #}
{% macro get_partition_by_keys(config) -%}
    {%- set partition_by_keys = config.get('partition_by', default=none) -%}
    {%- if partition_by_keys and partition_by_keys is string -%}
    {%- set partition_by_keys = [partition_by_keys] -%}
    {%- endif -%}
    {{ return(partition_by_keys) }}
{%- endmacro -%}
