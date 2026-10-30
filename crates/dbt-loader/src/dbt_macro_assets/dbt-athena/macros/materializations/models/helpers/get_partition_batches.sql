{% macro get_partition_batches(sql, as_subquery=True) -%}
    {# Retrieve partition configuration and set default partition limit #}
    {%- set partitioned_by = config.get('partitioned_by') -%}
    {%- set athena_partitions_limit = [1, config.get('partitions_limit', 100) | int] | max -%}
    {%- set partitioned_keys = adapter.format_partition_keys(partitioned_by) -%}
    {% do log('PARTITIONED KEYS: ' ~ partitioned_keys) %}

    {# Retrieve distinct partitions from the given SQL #}
    {% call statement('get_partitions', fetch_result=True) %}
        {%- if as_subquery -%}
            select distinct {{ partitioned_keys }} from ({{ sql }}) order by {{ partitioned_keys }};
        {%- else -%}
            select distinct {{ partitioned_keys }} from {{ sql }} order by {{ partitioned_keys }};
        {%- endif -%}
    {% endcall %}

    {# Initialize variables to store partition info #}
    {%- set table = load_result('get_partitions').table -%}
    {%- set rows = table.rows -%}
    {%- set ns = namespace(partitions = [], bucket_conditions = {}, bucket_numbers = [], bucket_column = None, is_bucketed = false) -%}

    {# Process each partition row #}
    {%- for row in rows -%}
        {%- set single_partition = [] -%}
        {# Use Namespace to hold the counter for loop index #}
        {%- set counter = namespace(value=0) -%}
        {# Loop through each column in the row #}
        {%- for col, partition_key in zip(row, partitioned_by) -%}
            {# Process bucketed columns using the new macro with the index #}
            {%- do process_bucket_column(col, partition_key, table, ns, counter.value) -%}

            {# Logic for non-bucketed columns #}
            {%- set bucket_match = modules.re.search('bucket\((.+?),\s*(\d+)\)', partition_key) -%}
            {%- if not bucket_match -%}
                {# For non-bucketed columns, format partition key and value #}
                {%- set column_type = adapter.convert_type(table, counter.value) -%}
                {%- set value, comp_func = adapter.format_value_for_partition(col, column_type) -%}
                {%- set partition_key_formatted = adapter.format_one_partition_key(partitioned_by[counter.value]) -%}
                {%- do single_partition.append(partition_key_formatted + comp_func + value) -%}
            {%- endif -%}
            {# Increment the counter #}
            {%- set counter.value = counter.value + 1 -%}
        {%- endfor -%}

        {# Concatenate conditions for a single partition #}
        {%- set single_partition_expression = single_partition | join(' and ') -%}
        {%- if single_partition_expression not in ns.partitions %}
            {%- do ns.partitions.append(single_partition_expression) -%}
        {%- endif -%}
    {%- endfor -%}

    {# Create conditions for each batch #}
    {%- set partitions_batches = [] -%}
    {%- if ns.is_bucketed -%}
        {%- set non_bucket_condition_chunks = [] -%}
        {%- set non_empty_partitions = ns.partitions | select | list -%}
        {%- set num_partition_values = non_empty_partitions | length -%}

        {# Coordinate chunk sizes so partition × bucket ≤ partitions_limit #}
        {%- if num_partition_values > 0 -%}
            {%- set partition_chunk_size = [num_partition_values, athena_partitions_limit] | min -%}
            {%- set bucket_chunk_size = [1, (athena_partitions_limit / partition_chunk_size) | int] | max -%}

            {%- for i in range(0, num_partition_values, partition_chunk_size) -%}
                {%- set batch = non_empty_partitions[i:i + partition_chunk_size] -%}
                {%- do non_bucket_condition_chunks.append(batch | join(' or ')) -%}
            {%- endfor -%}
        {%- else -%}
            {%- set bucket_chunk_size = athena_partitions_limit -%}
        {%- endif -%}

        {# Group bucket numbers so that partition_chunk × bucket_group ≤ limit.
           Values within the same bucket share one Iceberg partition, so grouping
           bucket numbers controls open-partition count.
           Values are still chunked by athena_partitions_limit to avoid exceeding
           Athena's query size limit. #}
        {%- for bi in range(0, ns.bucket_numbers | length, bucket_chunk_size) -%}
            {%- set bucket_group = ns.bucket_numbers[bi:bi + bucket_chunk_size] -%}
            {%- set all_values = [] -%}
            {%- for bn in bucket_group -%}
                {%- for v in ns.bucket_conditions[bn] -%}
                    {%- do all_values.append(v) -%}
                {%- endfor -%}
            {%- endfor -%}
            {%- for vi in range(0, all_values | length, athena_partitions_limit) -%}
                {%- set value_chunk = all_values[vi:vi + athena_partitions_limit] -%}
                {%- set bucket_cond = ns.bucket_column ~ " IN (" ~ value_chunk | join(", ") ~ ")" -%}
                {%- if non_bucket_condition_chunks | length > 0 -%}
                    {%- for pb in non_bucket_condition_chunks -%}
                        {%- do partitions_batches.append("(" ~ pb ~ ") and " ~ bucket_cond) -%}
                    {%- endfor -%}
                {%- else -%}
                    {%- do partitions_batches.append(bucket_cond) -%}
                {%- endif -%}
            {%- endfor -%}
        {%- endfor -%}
    {%- else -%}
        {# Non-bucketed: batch partitions respecting athena_partitions_limit #}
        {%- set non_empty_partitions = ns.partitions | select | list -%}
        {%- for i in range(0, non_empty_partitions | length, athena_partitions_limit) -%}
            {%- set batch = non_empty_partitions[i:i + athena_partitions_limit] -%}
            {%- do partitions_batches.append(batch | join(' or ')) -%}
        {%- endfor -%}
    {%- endif -%}
    {% do log('TOTAL PARTITIONS TO PROCESS: ' ~ partitions_batches | length) %}

    {{ return(partitions_batches) }}

{%- endmacro %}
