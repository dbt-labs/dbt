
-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro snowflake__get_merge_sql(target, source_sql, unique_key, dest_columns, incremental_predicates) -%}

    {#
       Workaround for Snowflake not being happy with a merge on a constant-false predicate.
       When no unique_key is provided, this macro will do a regular insert. If a unique_key
       is provided, then this macro will do a proper merge instead.
    #}

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute='name')) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {%- set dml -%}
    {%- if unique_key is none -%}

        {{ sql_header if sql_header is not none }}

        insert into {{ target }} ({{ dest_cols_csv }})
        (
            select {{ dest_cols_csv }}
            from {{ source_sql }}
        )

    {%- else -%}

        {{ default__get_merge_sql(target, source_sql, unique_key, dest_columns, incremental_predicates) }}

    {%- endif -%}
    {%- endset -%}

    {#-- Skip transaction wrapping for catalog-linked databases --#}
    {% if snowflake__is_catalog_linked_database(relation=config.model) %}
        {% do return(dml) %}
    {% else %}
        {% do return(snowflake_dml_explicit_transaction(dml)) %}
    {% endif %}

{% endmacro %}


-- funcsign: (string, string, string|list[string]|none, list[base_column], optional[list[string]]) -> string
{% macro snowflake__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {% set dml = default__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {#-- Skip transaction wrapping for catalog-linked databases --#}
    {% if snowflake__is_catalog_linked_database(relation=config.model) %}
        {% do return(dml) %}
    {% else %}
        {% do return(snowflake_dml_explicit_transaction(dml)) %}
    {% endif %}
{% endmacro %}


-- funcsign: (string, string, list[base_column]) -> string
{% macro snowflake__snapshot_merge_sql(target, source, insert_cols) %}
    {% set dml = default__snapshot_merge_sql(target, source, insert_cols) %}
    {#-- Skip transaction wrapping for catalog-linked databases --#}
    {% if snowflake__is_catalog_linked_database(relation=target) %}
        {% do return(dml) %}
    {% else %}
        {% do return(snowflake_dml_explicit_transaction(dml)) %}
    {% endif %}
{% endmacro %}


-- funcsign: (string) -> string
{% macro snowflake__get_incremental_append_sql(get_incremental_append_sql) %}
    {% set dml = default__get_incremental_append_sql(get_incremental_append_sql) %}
    {#-- Skip transaction wrapping for catalog-linked databases --#}
    {% if snowflake__is_catalog_linked_database(config.model) %}
        {% do return(dml) %}
    {% else %}
        {% do return(snowflake_dml_explicit_transaction(dml)) %}
    {% endif %}
{% endmacro %}


{% macro snowflake__get_incremental_microbatch_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}

    {#-- Add additional incremental_predicates to filter for batch --#}
    {% if model.batch and model.batch.event_time_start -%}
        {% do incremental_predicates.append("DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " >= to_timestamp_tz('" ~ model.config.__dbt_internal_microbatch_event_time_start ~ "')") %}
    {% endif %}
    {% if model.batch and model.batch.event_time_end -%}
        {% do incremental_predicates.append("DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " < to_timestamp_tz('" ~ model.config.__dbt_internal_microbatch_event_time_end ~ "')") %}
    {% endif %}
    {% do arg_dict.update({'incremental_predicates': incremental_predicates}) %}

    delete from {{ target }} DBT_INTERNAL_TARGET
    where (
    {% for predicate in incremental_predicates %}
        {%- if not loop.first %}and {% endif -%} {{ predicate }}
    {% endfor %}
    );

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
{% endmacro %}
