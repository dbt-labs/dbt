{% macro duckdb__get_incremental_microbatch_sql(arg_dict) -%}
    {# Extract and validate required config #}
    {%- set event_time = config.get('event_time') -%}
    {%- if not event_time -%}
        {{ exceptions.raise_compiler_error("microbatch incremental strategy requires an 'event_time' model config") }}
    {%- endif -%}

    {# microbatch is implemented as delete+insert on event_time; unique_key is ignored and misleading #}
    {%- set unique_key = config.get('unique_key') -%}
    {%- if unique_key -%}
        {{ exceptions.raise_compiler_error("microbatch incremental strategy does not support 'unique_key'. Microbatch runs delete+insert per batch based on 'event_time' and does not do key-based upserts. Remove 'unique_key' or use incremental_strategy='merge'.") }}
    {%- endif -%}

    {# Extract batch context - dbt sets these per batch run based on lookback window #}
    {%- set batch_ctx = model.get('batch') -%}
    {%- set batch_start = batch_ctx.get('event_time_start') if batch_ctx else none -%}
    {%- set batch_end = batch_ctx.get('event_time_end') if batch_ctx else none -%}

    {%- if not (batch_start and batch_end) -%}
        {{ exceptions.raise_compiler_error("microbatch incremental strategy requires 'batch.event_time_start' and 'batch.event_time_end' to be set in the context") }}
    {%- endif -%}

    {# Extract remaining arguments #}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = normalize_incremental_predicates(arg_dict.get("incremental_predicates")) -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {# Build the batch time filter predicate #}
    {# batch_start and batch_end are already UTC timestamps from dbt Python code #}
    {%- set batch_predicate -%}
        {{ event_time }} >= '{{ batch_start }}'
        and {{ event_time }} < '{{ batch_end }}'
    {%- endset -%}

    {# Build combined WHERE clause with optional incremental predicates #}
    {%- set where_clause -%}
        {{ batch_predicate }}
        {%- for predicate in incremental_predicates %}
        and ({{ predicate }})
        {%- endfor %}
    {%- endset -%}

    {# Generate delete + insert SQL #}
    {%- set build_sql -%}
        delete from {{ target }}
        where {{ where_clause }};

        insert into {{ target }} ({{ dest_cols_csv }})
        select {{ dest_cols_csv }}
        from {{ source }}
        where {{ batch_predicate }};
    {%- endset -%}

    {{ return(build_sql) }}
{%- endmacro %}
