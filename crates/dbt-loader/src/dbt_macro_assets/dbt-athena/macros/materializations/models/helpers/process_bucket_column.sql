{% macro process_bucket_column(col, partition_key, table, ns, col_index) %}
    {# Extract bucket information from the partition key #}
    {%- set bucket_match = modules.re.search('bucket\((.+?),\s*(\d+)\)', partition_key) -%}

    {%- if bucket_match -%}
        {# For bucketed columns, compute bucket numbers and conditions #}
        {%- set column_type = adapter.convert_type(table, col_index) -%}
        {%- set ns.is_bucketed = true -%}
        {%- set ns.bucket_column = bucket_match[1] -%}
        {%- set bucket_num = adapter.murmur3_hash(col, bucket_match[2] | int) -%}
        {%- set formatted_value, comp_func = adapter.format_value_for_partition(col, column_type) -%}

        {%- if bucket_num not in ns.bucket_numbers %}
            {%- do ns.bucket_numbers.append(bucket_num) %}
            {%- do ns.bucket_conditions.update({bucket_num: [formatted_value]}) -%}
        {%- elif formatted_value not in ns.bucket_conditions[bucket_num] %}
            {%- do ns.bucket_conditions[bucket_num].append(formatted_value) -%}
        {%- endif -%}
    {%- endif -%}
{% endmacro %}
