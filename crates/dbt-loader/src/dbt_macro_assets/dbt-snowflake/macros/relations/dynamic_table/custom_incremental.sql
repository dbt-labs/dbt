{#-
    Custom incremental dynamic tables (CI DT): a dynamic table whose refresh is one DML statement
    against SELF, `create dynamic table X (<columns>) ... refresh using (<dml>)`.
    https://docs.snowflake.com/en/user-guide/dynamic-tables/custom-incrementalization

    A model opts in with `refresh_mode='CUSTOM_INCREMENTAL'` and picks the DML with
    `incremental_strategy`. The model's SELECT reads its change source with `{{ changes(...) }}`.
-#}

-- funcsign: () -> bool
{% macro snowflake__dynamic_table_is_custom_incremental() -%}
    {{ return((config.get('refresh_mode') or '') | upper == 'CUSTOM_INCREMENTAL') }}
{%- endmacro %}


-- funcsign: () -> list[string]
{% macro snowflake__custom_incremental_strategies() -%}
    {#- Each strategy has a `snowflake__validate_custom_incremental_<strategy>_config` check and a
        branch in `snowflake__custom_incremental_refresh_using_sql`. -#}
    {{ return(['merge']) }}
{%- endmacro %}


-- funcsign: () -> string
{% macro snowflake__custom_incremental_strategy() -%}
    {{ return((config.get('incremental_strategy') or '') | lower) }}
{%- endmacro %}


-- funcsign: () -> string
{% macro snowflake__validate_custom_incremental_config() -%}
    {%- set strategy = snowflake__custom_incremental_strategy() -%}
    {%- set strategies = snowflake__custom_incremental_strategies() -%}

    {%- if not strategy -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` has refresh_mode='CUSTOM_INCREMENTAL' but no `incremental_strategy`. Supported strategies: '" ~ strategies | join("', '") ~ "'."
        ) %}
    {%- elif strategy not in strategies -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` has refresh_mode='CUSTOM_INCREMENTAL' with unsupported incremental_strategy '" ~ strategy ~ "'. Supported strategies: '" ~ strategies | join("', '") ~ "'."
        ) %}
    {%- endif -%}

    {%- if adapter.build_catalog_relation(config.model).catalog_type == 'BUILT_IN' -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` has refresh_mode='CUSTOM_INCREMENTAL', which Snowflake does not support for dynamic Iceberg tables."
        ) %}
    {%- endif -%}

    {%- if config.get('immutable_where') -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` sets `immutable_where`, which Snowflake does not support on a custom incremental dynamic table (REFRESH USING)."
        ) %}
    {%- endif -%}

    {#- `{{ this }}` renders the model's own name. Inside `refresh using` Snowflake only accepts the
        table as SELF, so reject it here with a clear message. -#}
    {%- set this_pattern = (this | string) | replace('.', '\\.') | replace('$', '\\$') -%}
    {%- if modules.re.search('(^|[^A-Za-z0-9_$."])' ~ this_pattern ~ '($|[^A-Za-z0-9_$"])', sql, modules.re.IGNORECASE) -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` references itself (`{{ this }}`), which custom incremental dynamic tables do not support yet: Snowflake only lets the refresh body refer to the table as SELF. Remove the reference; rendering `{{ this }}` as SELF is planned."
        ) %}
    {%- endif -%}

    {%- if strategy == 'merge' -%}
        {% do snowflake__validate_custom_incremental_merge_config() %}
    {%- endif -%}
{%- endmacro %}


-- funcsign: () -> string
{% macro snowflake__validate_custom_incremental_merge_config() -%}
    {%- if not config.get('unique_key') -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "` uses incremental_strategy='merge' with refresh_mode='CUSTOM_INCREMENTAL', which requires `unique_key`."
        ) %}
    {%- endif -%}

    {%- set delete_condition = config.get('delete_condition') -%}
    {%- if delete_condition and 'dbt_internal_dest' in delete_condition | lower -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ this ~ "`: with incremental_strategy='merge', `delete_condition` is tested against the incoming row, so it must reference DBT_INTERNAL_SOURCE, not DBT_INTERNAL_DEST."
        ) %}
    {%- endif -%}
{%- endmacro %}


-- funcsign: (relation) -> list[base_column]
{% macro snowflake__custom_incremental_discover_columns(relation) -%}
{#-
    Learn the model's output columns, which a CI DT must declare because Snowflake cannot infer
    them from DML. The model is rendered again with `changes()` in its discovery form (unbounded
    CHANGES only runs inside a refresh), run zero-row into a temporary table, and described.
-#}
    {%- set raw_code = model.get('raw_code') or '' -%}
    {%- if modules.re.search('changes\\s*\\(\\s*information\\s*=>', raw_code, modules.re.IGNORECASE) -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ relation ~ "` reads CHANGES(INFORMATION => ...) directly. dbt cannot learn the columns of a custom incremental dynamic table from raw CHANGES; read the source with `{{ changes(ref('...')) }}` or `{{ changes(source('...', '...')) }}` instead."
        ) %}
    {%- endif -%}

    {%- do model.update({'_dbt_changes_discovery': true}) -%}
    {%- set discovery_sql = render(raw_code) -%}
    {%- do model.update({'_dbt_changes_discovery': false}) -%}
    {%- if '__dbt__cte__' in discovery_sql -%}
        {% do exceptions.raise_compiler_error(
            "Dynamic table `" ~ relation ~ "` refs an ephemeral model, which custom incremental dynamic tables do not support yet. Materialize the upstream model as a view or table."
        ) %}
    {%- endif -%}

    {%- set tmp_relation = make_temp_relation(relation).incorporate(type='table') -%}
    {% call statement('custom_incremental_discovery') -%}
        {{ create_table_as(True, tmp_relation, 'select * from (\n' ~ discovery_sql ~ '\n) limit 0') }}
    {%- endcall %}
    {%- set columns = adapter.get_columns_in_relation(tmp_relation) -%}
    {% do adapter.drop_relation(tmp_relation) %}

    {%- for column in columns -%}
        {%- if column.name | upper in ['METADATA$ACTION', 'METADATA$ISUPDATE', 'METADATA$ROW_ID'] -%}
            {% do exceptions.raise_compiler_error(
                "Dynamic table `" ~ relation ~ "` outputs a column named " ~ column.name ~ ", which a custom incremental dynamic table cannot store. Alias it in the model's SELECT."
            ) %}
        {%- endif -%}
    {%- endfor -%}

    {{ return(columns) }}
{%- endmacro %}


-- funcsign: (list[base_column]) -> string
{% macro snowflake__custom_incremental_columns_ddl(columns) -%}
    ({%- for column in columns -%}
        {{ column.quoted }} {{ column.data_type }}{% if not loop.last %}, {% endif %}
    {%- endfor -%})
{%- endmacro %}


-- funcsign: (string, list[base_column]) -> string
{% macro snowflake__custom_incremental_refresh_using_sql(sql, columns) -%}
    {%- set strategy = snowflake__custom_incremental_strategy() -%}
    {%- if strategy == 'merge' -%}
        {{ snowflake__custom_incremental_merge_sql(sql, columns) }}
    {%- else -%}
        {% do exceptions.raise_compiler_error("Unexpected custom incremental strategy '" ~ strategy ~ "' for " ~ this) %}
    {%- endif -%}
{%- endmacro %}


-- funcsign: (string, list[base_column]) -> string
{% macro snowflake__custom_incremental_merge_sql(sql, columns) -%}
{#-
    The model SQL is wrapped in a subquery: a CI DT body cannot start with WITH (a syntax error),
    while a WITH inside the USING subquery is accepted.
    `delete_condition` is tested against the incoming row: a matched row it holds for is deleted,
    and an unmatched one is not inserted.
    `merge_update_columns` / `merge_exclude_columns` narrow the updated columns, as in the
    incremental merge.
-#}
    {%- set unique_key = config.get('unique_key') -%}
    {%- set unique_keys = [unique_key] if unique_key is string else unique_key -%}
    {%- set delete_condition = config.get('delete_condition') -%}
    {%- set column_names = columns | map(attribute='quoted') | list -%}
    {%- set update_columns = get_merge_update_columns(config.get('merge_update_columns'), config.get('merge_exclude_columns'), columns) -%}
merge into SELF as DBT_INTERNAL_DEST
using (
{{ sql }}
) as DBT_INTERNAL_SOURCE
on {% for key in unique_keys %}DBT_INTERNAL_DEST.{{ key }} = DBT_INTERNAL_SOURCE.{{ key }}{% if not loop.last %} and {% endif %}{% endfor %}
{%- if delete_condition %}
when matched and coalesce({{ delete_condition }}, false) then delete
{%- endif %}
when matched then update set {% for name in update_columns %}{{ name }} = DBT_INTERNAL_SOURCE.{{ name }}{% if not loop.last %}, {% endif %}{% endfor %}
when not matched{% if delete_condition %} and not coalesce({{ delete_condition }}, false){% endif %} then insert ({{ column_names | join(', ') }}) values ({% for name in column_names %}DBT_INTERNAL_SOURCE.{{ name }}{% if not loop.last %}, {% endif %}{% endfor %})
{%- endmacro %}


-- funcsign: (any) -> bool
{% macro snowflake__dynamic_table_kind_changed(described) -%}
{#-
    Whether the existing dynamic table and the model disagree on being a CI DT. Neither direction
    can be altered in place, and the refresh_mode diff misses CI DT -> regular because it treats a
    desired AUTO as matching anything.
-#}
    {%- set rows = described['dynamic_table'].rows -%}
    {%- set existing_mode = (rows[0]['refresh_mode'] if rows | length > 0 else '') or '' -%}
    {%- set existing_is_custom = existing_mode | upper == 'CUSTOM_INCREMENTAL' -%}
    {{ return(existing_is_custom != snowflake__dynamic_table_is_custom_incremental()) }}
{%- endmacro %}
