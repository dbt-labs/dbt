{#- MariaDB rejects an alias on single-table DELETE, and the multi-table form
    with an alias fails without a default database. So delete without an
    alias, and point DBT_INTERNAL_DEST references in incremental_predicates at
    the fully qualified target. -#}
{% macro mariadb__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) -%}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is string %}
        {% set unique_key = [unique_key] %}
        {% endif %}

        {%- set unique_key_str = unique_key|join(', ') -%}

        delete from {{ target }}
        where ({{ unique_key_str }}) in (
            select distinct {{ unique_key_str }}
            from {{ source }} as DBT_INTERNAL_SOURCE
        )
        {%- if incremental_predicates %}
            {% for predicate in incremental_predicates %}
                and {{ predicate | replace('DBT_INTERNAL_DEST.', target ~ '.') }}
            {% endfor %}
        {%- endif -%};

    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )

{%- endmacro %}


{#- Microbatch: delete the batch window from the target, then insert the
    batch. The UTC rfc3339 batch boundaries become 'YYYY-MM-DD HH:MM:SS'
    literals, matching the Rust event-time filter rendering for MariaDB. -#}
{% macro mariadb__get_incremental_microbatch_sql(arg_dict) -%}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set predicates = [] -%}
    {%- for predicate in arg_dict.get('incremental_predicates') or [] -%}
        {%- do predicates.append(predicate | replace('DBT_INTERNAL_DEST.', target ~ '.')) -%}
    {%- endfor -%}

    {%- if model.batch and model.batch.event_time_start -%}
        {%- set start_ts = model.config.__dbt_internal_microbatch_event_time_start | replace('T', ' ') | replace('+00:00', '') | replace('Z', '') -%}
        {%- do predicates.append(model.config.event_time ~ " >= '" ~ start_ts ~ "'") -%}
    {%- endif -%}
    {%- if model.batch and model.batch.event_time_end -%}
        {%- set end_ts = model.config.__dbt_internal_microbatch_event_time_end | replace('T', ' ') | replace('+00:00', '') | replace('Z', '') -%}
        {%- do predicates.append(model.config.event_time ~ " < '" ~ end_ts ~ "'") -%}
    {%- endif -%}

    {%- if predicates %}
    delete from {{ target }}
    where {{ predicates | join(' and ') }};
    {% endif %}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) %}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
{%- endmacro %}
