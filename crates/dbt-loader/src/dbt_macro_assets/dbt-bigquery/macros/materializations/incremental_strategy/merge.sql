{#
    Override only the unique-key match used inside the MERGE ON predicate.

    When the `enable_truthy_nulls_equals_macro` flag is enabled, `bigquery__equals`
    emits `IS NOT DISTINCT FROM`. Inside a MERGE on a partitioned table that has
    `require_partition_filter=True`, BigQuery's partition-pruning analyzer no longer
    recognizes the `(<partition_field> is null or <partition_field> is not null)`
    auxiliary predicate (added by `predicate_for_avoid_require_partition_filter`)
    as a valid partition filter, and the MERGE fails at runtime. Use the
    equivalent `(a is null and b is null) or (a = b)` form instead so partition
    pruning still works.
#}
{% macro bigquery__get_merge_unique_key_match(source_unique_key, target_unique_key) -%}
    {%- if adapter.behavior.enable_truthy_nulls_equals_macro.no_warn -%}
        (({{ source_unique_key }} is null and {{ target_unique_key }} is null) or ({{ source_unique_key }} = {{ target_unique_key }}))
    {%- else -%}
        ({{ source_unique_key }} = {{ target_unique_key }})
    {%- endif %}
{%- endmacro %}


{% macro bq_generate_incremental_merge_build_sql(
    tmp_relation, target_relation, sql, unique_key, partition_by, dest_columns, tmp_relation_exists, incremental_predicates
) %}
    {%- set source_sql -%}
        {%- if tmp_relation_exists -%}
        (
        select
        {% if partition_by.time_ingestion_partitioning -%}
        {{ partition_by.insertable_time_partitioning_field() }},
        {%- endif -%}
        * from {{ tmp_relation }}
        )
        {%- else -%} {#-- wrap sql in parens to make it a subquery --#}
        (
            {%- if partition_by.time_ingestion_partitioning -%}
            {{ wrap_with_time_ingestion_partitioning_sql(partition_by, sql, True) }}
            {%- else -%}
            {{sql}}
            {%- endif %}
        )
        {%- endif -%}
    {%- endset -%}

    {%- set predicates = [] if incremental_predicates is none else [] + incremental_predicates -%}
    {%- set avoid_require_partition_filter = predicate_for_avoid_require_partition_filter() -%}
    {%- if avoid_require_partition_filter is not none -%}
        {% do predicates.append(avoid_require_partition_filter) %}
    {%- endif -%}

    {% set build_sql = get_merge_sql(target_relation, source_sql, unique_key, dest_columns, predicates) %}

    {{ return(build_sql) }}

{% endmacro %}
