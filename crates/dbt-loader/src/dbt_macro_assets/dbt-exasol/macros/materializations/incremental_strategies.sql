{% macro exasol__get_incremental_microbatch_sql(arg_dict) %}
    {#- Microbatch: DELETE the batch window from the target, then INSERT the
        batch from the temp relation. Exasol TIMESTAMP literals accept no
        time-zone offset, so the UTC rfc3339 batch boundaries are rewritten to
        plain TIMESTAMP literals (offset stripped), matching the Rust
        event-time filter rendering for Exasol. -#}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}

    {% if model.batch and model.batch.event_time_start -%}
        {%- set start_ts = model.config.__dbt_internal_microbatch_event_time_start | replace('T', ' ') | replace('+00:00', '') | replace('Z', '') -%}
        {% do incremental_predicates.append(model.config.event_time ~ " >= TIMESTAMP '" ~ start_ts ~ "'") %}
    {% endif %}
    {% if model.batch and model.batch.event_time_end -%}
        {%- set end_ts = model.config.__dbt_internal_microbatch_event_time_end | replace('T', ' ') | replace('+00:00', '') | replace('Z', '') -%}
        {% do incremental_predicates.append(model.config.event_time ~ " < TIMESTAMP '" ~ end_ts ~ "'") %}
    {% endif %}

    {% if incremental_predicates %}
    delete from {{ target }}
    where (
    {% for predicate in incremental_predicates %}
        {%- if not loop.first %}and {% endif -%} {{ predicate }}
    {% endfor %}
    );
    {% endif %}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
{% endmacro %}
