{#
    The Snowflake INSERT OVERWRITE instruction is not a partition‐targeted update like in Spark or
    the write disposition options in BigQuery. In Snowflake, specifying OVERWRITE causes the entire
    target table to be cleared (essentially a TRUNCATE) before the new data is inserted in one atomic
    operation. That means every time running an INSERT OVERWRITE means discarding all existing data
    and replacing it wholesale.

    But because dbt has very specific logic for dropping tables, we do not add this method as a table
    option. It would complicate that mental model tremendously. We instead pass this caveat about this
    so-called incremental strategy to the user.

    https://github.com/dbt-labs/dbt-adapters/issues/736#issuecomment-2640918081
#}

{#
    DIVERGENCE:
    This macro was named as `get_incremental_insert_overwrite_sql` in the dbt-snowflake project
    https://github.com/dbt-labs/dbt-adapters/blob/906472ee65ab2438cc4f7142f4a9866366cea9e1/dbt-snowflake/src/dbt/include/snowflake/macros/materializations/incremental/insert_overwrite.sql#L15
    This `get_incremental_insert_overwrite_sql` also exists in the dbt-adapters project
    With the previous name, since `adapter.get_incremental_strategy_macro` returns a DispatchObject - its supposed to be a dialect specific macro that can be dispatched to from the above macro,
    This is also due to fs doesnt support `context` globals, adapter.dispatch will not work as expected for this case.
    Therefore, we rename the macro to `snowflake__get_incremental_insert_overwrite_sql` as a workaround.
#}
-- funcsign: (struct{ target_relation: relation, temp_relation: relation, unique_key: optional[string], dest_columns: list[base_column], incremental_predicates: optional[string]}) -> string
{% macro snowflake__get_incremental_insert_overwrite_sql(arg_dict) -%}
  {{ adapter.dispatch('insert_overwrite_get_sql', 'dbt')(arg_dict["target_relation"], arg_dict["temp_relation"], arg_dict["unique_key"], arg_dict["dest_columns"]) }}
{%- endmacro %}

-- funcsign: (relation, relation, optional[string], list[base_column]) -> string
{% macro snowflake__insert_overwrite_get_sql(target, source, unique_key, dest_columns) -%}

    {%- set dml -%}

    {%- set overwrite_columns = config.get('overwrite_columns', []) -%}

    {{ config.get('sql_header', '') }}

    {% set target_columns_list = '(' ~ ', '.join(overwrite_columns) ~ ')' if overwrite_columns else '' %}
    {% set source_query_columns_list = ', '.join(overwrite_columns) if overwrite_columns else '*' %}
    insert overwrite into {{ target.render() }} {{ target_columns_list }}
        select {{ source_query_columns_list }}
        from {{ source.render() }}

    {%- endset -%}

    {#-- Skip transaction wrapping for catalog-linked databases --#}
    {% if snowflake__is_catalog_linked_database(relation=config.model) %}
        {% do return(dml) %}
    {% else %}
        {% do return(snowflake_dml_explicit_transaction(dml)) %}
    {% endif %}

{% endmacro %}
