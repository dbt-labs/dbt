{% macro sqlserver__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) %}
  {{ default__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) }}
  {{ get_query_options(parse_options=True) }}
{% endmacro %}

{% macro sqlserver__get_insert_overwrite_merge_sql(target, source, dest_columns, predicates, include_sql_header) %}
  {{ default__get_insert_overwrite_merge_sql(target, source, dest_columns, predicates, include_sql_header) }};
{% endmacro %}

{% macro sqlserver__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) %}

    {% set query_label = get_query_options(parse_options=True) %}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not string %}
            SET NOCOUNT ON;
            delete from {{ target }}
            where exists (
                select null
                from {{ source }}
                where
                {% for key in unique_key %}
                    {{ source }}.{{ key }} = {{ target }}.{{ key }}
                    {{ "and " if not loop.last }}
                {% endfor %}
            )
            {% if incremental_predicates %}
                {% for predicate in incremental_predicates %}
                    and {{ predicate }}
                {% endfor %}
            {% endif %}
            {{ query_label }}
            SET NOCOUNT OFF;
        {% else %}
            SET NOCOUNT ON;
            delete from {{ target }}
            where (
                {{ unique_key }}) in (
                select ({{ unique_key }})
                from {{ source }}
            )
            {%- if incremental_predicates %}
                {% for predicate in incremental_predicates %}
                    and {{ predicate }}
                {% endfor %}
            {%- endif -%}
            {{ query_label }}
            SET NOCOUNT OFF;
        {% endif %}
    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    ){{ query_label }}
{% endmacro %}

{% macro sqlserver__get_incremental_microbatch_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}
    {%- set query_label = get_query_options(parse_options=True) -%}

    {#-- Add additional incremental_predicates to filter for batch --#}
    {% if model.config.get("__dbt_internal_microbatch_event_time_start") -%}
    {{ log("incremental append event start time > DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " >= cast('" ~ model.config.__dbt_internal_microbatch_event_time_start ~ "' as datetimeoffset)") }}
        {% do incremental_predicates.append("DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " >= cast('" ~ model.config.__dbt_internal_microbatch_event_time_start ~ "' as datetimeoffset)") %}
    {% endif %}
    {% if model.config.__dbt_internal_microbatch_event_time_end -%}
    {{ log("incremental append event end time < DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " < cast('" ~ model.config.__dbt_internal_microbatch_event_time_end ~ "' as datetimeoffset)") }}
        {% do incremental_predicates.append("DBT_INTERNAL_TARGET." ~ model.config.event_time ~ " < cast('" ~ model.config.__dbt_internal_microbatch_event_time_end ~ "' as datetimeoffset)") %}
    {% endif %}
    {% do arg_dict.update({'incremental_predicates': incremental_predicates}) %}

    SET NOCOUNT ON;
    delete DBT_INTERNAL_TARGET from {{ target }} AS DBT_INTERNAL_TARGET
    where (
    {% for predicate in incremental_predicates %}
        {%- if not loop.first %}and {% endif -%} {{ predicate }}
    {% endfor %}
    )
    {{ query_label }}
    SET NOCOUNT OFF;

    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}
    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    )
    {{ query_label }}
{% endmacro %}
