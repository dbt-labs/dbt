{#-
    The gate for this path -- behavior flag on + native (info schema) catalog -- lives in Rust as
    adapter.dynamic_table_create_or_alter_enabled(), so the decision is unit-testable without a
    database. The materialization calls it; this file only renders the DDL once that gate has
    passed. The gate admits native (INFO_SCHEMA) and managed Iceberg (BUILT_IN) dynamic tables.
-#}
-- funcsign: (relation, relation, string) -> string
{% macro snowflake__get_create_or_alter_dynamic_table_sql(existing_relation, target_relation, sql) -%}
{#-
    Definition SQL for the query-evolution path (assumes the gate above already passed).

    CREATE OR ALTER cannot convert a regular dynamic table to or from a dynamic Iceberg table, so a
    mismatch between the live relation's format and the configured catalog is rebuilt with
    CREATE OR REPLACE before anything else is considered.

    CREATE OR ALTER expresses the complete desired end state and applies it idempotently,
    preserving the object -- and its grants / attached policies / tags -- instead of replacing it.
    This is how a native dynamic table deploys SQL edits without --full-refresh.

    The config changeset does NOT track the query, so a `none` changeset means "query-only edit
    (or no change)" and must still emit CREATE OR ALTER -- that is the query-evolution fix. We
    consult the changeset only to:
      1. route changes dbt flags as requiring a full rebuild (`requires_full_refresh`: transient
         and refresh_mode) to CREATE OR REPLACE -- transient in particular cannot be converted by
         CREATE OR ALTER; and
      2. honor on_configuration_change (continue / fail) for other detected config changes.

    See the DDL macro below for the per-attribute contract. Note CREATE OR ALTER also cannot
    express every schema change (column reorder, mid-list insert, type narrowing, drop-all); those
    surface a Snowflake error at run time and the fallback is --full-refresh.
-#}
    {%- set configuration_changes = snowflake__get_dynamic_table_configuration_changes(existing_relation, config) -%}
    {%- set on_configuration_change = config.get('on_configuration_change') -%}
    {%- set dynamic_table = target_relation.from_config(config.model) -%}
    {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}
    {%- set target_is_iceberg = catalog_relation.catalog_type == 'BUILT_IN' -%}

    {%- if existing_relation.is_iceberg_format != target_is_iceberg -%}
        {{ return(get_replace_sql(existing_relation, target_relation, sql)) }}

    {%- elif configuration_changes is none -%}
        {#- query-only edit (or no change): sync the definition in place. -#}
        {{ return(snowflake__create_or_alter_dynamic_table_sql(dynamic_table, target_relation, sql)) }}

    {%- elif on_configuration_change == 'apply' -%}
        {%- if configuration_changes.requires_full_refresh -%}
            {#- transient / refresh_mode: dbt flags these as requires_full_refresh -> rebuild. -#}
            {{ return(get_replace_sql(existing_relation, target_relation, sql)) }}
        {%- else -%}
            {#- CREATE OR ALTER declares the full end state, applying config changes and query together. -#}
            {{ return(snowflake__create_or_alter_dynamic_table_sql(dynamic_table, target_relation, sql)) }}
        {%- endif -%}

    {%- elif on_configuration_change == 'continue' -%}
        {#- Known limitation: a run carrying BOTH a tracked config change and a SQL edit skips both
            here (the changeset can't see the query, so we can't apply the edit while honoring
            'continue' for the config change). Documented in the flag description. -#}
        {{ exceptions.warn("Configuration changes were identified and `on_configuration_change` was set to `continue` for `" ~ target_relation ~ "`") }}
        {{ return('') }}
    {%- elif on_configuration_change == 'fail' -%}
        {{ exceptions.raise_fail_fast_error("Configuration changes were identified and `on_configuration_change` was set to `fail` for `" ~ target_relation ~ "`") }}
    {%- else -%}
        {{ exceptions.raise_compiler_error("Unexpected configuration scenario: `" ~ on_configuration_change ~ "`") }}
    {%- endif -%}
{%- endmacro %}


{% macro snowflake__create_or_alter_dynamic_table_sql(dynamic_table, relation, sql) -%}
    {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

    {%- if catalog_relation.catalog_type == 'INFO_SCHEMA' -%}
        {{ snowflake__create_or_alter_dynamic_table_info_schema_sql(dynamic_table, relation, sql) }}
    {%- elif catalog_relation.catalog_type == 'BUILT_IN' -%}
        {{ snowflake__create_or_alter_dynamic_table_built_in_sql(dynamic_table, relation, catalog_relation, sql) }}
    {%- else -%}
        {% do exceptions.raise_compiler_error('Unexpected model config for: ' ~ relation) %}
    {%- endif -%}
{%- endmacro %}


{% macro snowflake__create_or_alter_dynamic_table_info_schema_sql(dynamic_table, relation, sql) -%}
{#-
    Renders the CREATE OR ALTER DDL for a native (info schema) dynamic table.

    Attribute contract -- CREATE OR ALTER is declarative, so every run re-declares the full desired
    state. What that means per attribute (and why some clauses that CREATE/REPLACE emit are absent):

      Emitted here (dbt config is the source of truth; omitting one resets it to its
      default/inherited value, exactly as CREATE OR REPLACE would):
        transient, target_lag, warehouse, initialization_warehouse, refresh_mode,
        scheduler, cluster_by, immutable_where

      Never emitted, by design:
        initialize   -- create-time-only; Snowflake prohibits it on an existing table
        copy grants  -- copies grants from a *replaced* object; CREATE OR ALTER replaces nothing
                        (existing grants persist across it anyway)
        row_access_policy, table_tag (and masking / projection / aggregation policies)
                     -- CREATE OR ALTER rejects policy & tag clauses (Snowflake error 001506).
                        Snowflake preserves the existing ones across the statement. A config-side
                        change to them is not applied on this path -- use --full-refresh to
                        (re)apply.

    transient / refresh_mode: when dbt DETECTS a change to these it flags requires_full_refresh and
    the caller routes to CREATE OR REPLACE first (transient cannot be converted by CREATE OR ALTER
    regardless). Known limitation: when `transient` is unset in config, dbt does not compare it, so
    if the effective transient default diverges from the live table (default flag changed, or an
    explicit `transient` was removed) this macro re-declares transient and Snowflake rejects the
    in-place flip (error 001521). The documented recovery is --full-refresh. (A refresh_mode change
    to AUTO is likewise undetected, but CREATE OR ALTER applies it in place -- AUTO resolves to a
    concrete mode -- so it is not an error.) See the flag description in behavior.rs.

    Args:
    - dynamic_table: the dynamic table config built from the model (relation.from_config)
    - relation: the target relation
    - sql: str - the code defining the model
-#}

    {#- Determine transient: explicit config takes precedence, otherwise use behavior flag default -#}
    {%- if dynamic_table.transient is not none -%}
        {%- set is_transient = dynamic_table.transient -%}
    {%- elif adapter.behavior.snowflake_default_transient_dynamic_tables.no_warn -%}
        {%- set is_transient = true -%}
    {%- else -%}
        {%- set is_transient = false -%}
    {%- endif -%}
    {%- set transient_keyword = 'transient ' if is_transient else '' -%}

create or alter {{ transient_keyword }}dynamic table {{ relation }}
    {% if dynamic_table.target_lag is not none %}target_lag = '{{ dynamic_table.target_lag }}'{% endif %}
    warehouse = {{ dynamic_table.snowflake_warehouse }}
    {{ optional('initialization_warehouse', dynamic_table.snowflake_initialization_warehouse) }}
    {{ optional('refresh_mode', dynamic_table.refresh_mode) }}
    {#- initialize is a create-time-only attribute; CREATE OR ALTER always runs on an existing
        table where it is meaningless and Snowflake prohibits changing it, so we omit it. -#}
    {% if dynamic_table.scheduler is not none %}
    scheduler = '{{ dynamic_table.scheduler }}'
    {% elif dynamic_table.target_lag is none %}
    scheduler = 'DISABLE'
    {% endif %}
    {{ optional('cluster by', dynamic_table.cluster_by, quote_char='(', equals_char='') }}
    {{ optional('immutable where', dynamic_table.immutable_where, quote_char='(', equals_char='') }}
    as (
        {{ sql }}
    )

{%- endmacro %}


{% macro snowflake__create_or_alter_dynamic_table_built_in_sql(dynamic_table, relation, catalog_relation, sql) -%}
{#-
    Renders the CREATE OR ALTER DDL for a managed dynamic Iceberg table (Snowflake is the Iceberg
    catalog).

    The dynamic-table clauses follow the same rules as a native dynamic table (see the info schema
    macro above). The Iceberg identity clauses -- external volume, catalog, base location, Iceberg
    version -- cannot change: restating the current value is accepted and any difference fails with
    "CREATE OR ALTER on Dynamic Iceberg Table with changed iceberg properties", so they are emitted
    exactly as CREATE OR REPLACE would. Editing one of them requires --full-refresh.

    Not emitted: transient (Iceberg targets reject it), initialize, copy grants, row access policy
    and tag -- for the same reasons as the native macro.
-#}
create or alter dynamic iceberg table {{ relation }}
    {% if dynamic_table.target_lag is not none %}target_lag = '{{ dynamic_table.target_lag }}'{% endif %}
    warehouse = {{ dynamic_table.snowflake_warehouse }}
    {{ optional('initialization_warehouse', dynamic_table.snowflake_initialization_warehouse) }}
    {{ optional('external_volume', catalog_relation.external_volume, "'") }}
    catalog = 'SNOWFLAKE'
    {{ optional('base_location', catalog_relation.base_location, "'") }}
    {{ optional('iceberg_version', catalog_relation.iceberg_version) }}
    {{ optional('refresh_mode', dynamic_table.refresh_mode) }}
    {% if dynamic_table.scheduler is not none %}
    scheduler = '{{ dynamic_table.scheduler }}'
    {% elif dynamic_table.target_lag is none %}
    scheduler = 'DISABLE'
    {% endif %}
    {{ optional('cluster by', dynamic_table.cluster_by, quote_char='(', equals_char='') }}
    {{ optional('immutable where', dynamic_table.immutable_where, quote_char='(', equals_char='') }}
    as (
        {{ sql }}
    )

{%- endmacro %}
