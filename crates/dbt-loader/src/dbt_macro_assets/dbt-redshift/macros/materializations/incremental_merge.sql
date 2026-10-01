{% macro redshift__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) -%}
    {%- set predicates = _update_predicates(target, incremental_predicates) -%}

    {%- set merge_update_columns = config.get('merge_update_columns') -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns') -%}
    {%- set update_columns = get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) -%}
    {%- set insert_columns = get_merge_update_columns(none, none, dest_columns) -%}
    {%- set sql_header = config.get('sql_header', none) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
            {% for key in unique_key %}
                {% set this_key_match %}
                    DBT_INTERNAL_SOURCE.{{ key }} = {{ target }}.{{ key }}
                {% endset %}
                {% do predicates.append(this_key_match) %}
            {% endfor %}
        {% else %}
            {% set unique_key_match %}
                DBT_INTERNAL_SOURCE.{{ unique_key }} = {{ target }}.{{ unique_key }}
            {% endset %}
            {% do predicates.append(unique_key_match) %}
        {% endif %}
    {% else %}
        {% do predicates.append('FALSE') %}
    {% endif %}

    {{ sql_header if sql_header is not none }}

    merge into {{ target }}
        using {{ source }} as DBT_INTERNAL_SOURCE
        on {{"(" ~ predicates | join(") and (") ~ ")"}}

    {% if unique_key %}
    when matched then update set
        {% for column_name in update_columns -%}
            {{ column_name }} = DBT_INTERNAL_SOURCE.{{ column_name }}
            {%- if not loop.last %}, {% endif %}
        {% endfor %}
    {% endif %}

    when not matched then insert (
        {% for column_name in insert_columns -%}
            {{ column_name }}
            {%- if not loop.last %}, {% endif %}
        {% endfor %}
    )
    values (
        {% for column_name in insert_columns -%}
            DBT_INTERNAL_SOURCE.{{ column_name }}
            {%- if not loop.last %}, {% endif %}
        {% endfor %}
    )

{% endmacro %}

{% macro redshift__get_incremental_microbatch_sql(arg_dict) %}
    {#-
        Technically this function could just call out to the default implementation of delete_insert.
        However, the default implementation requires a unique_id, which we actually do not want or
        need. Thus we re-implement delete insert here without the unique_id requirement
    -#}

    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set predicates = [] -%}

    {%- set incremental_predicates = [] if arg_dict.get('incremental_predicates') is none else arg_dict.get('incremental_predicates') -%}
    {%- for pred in incremental_predicates -%}
        {% if "DBT_INTERNAL_DEST." in pred %}
            {%- set pred =  pred | replace("DBT_INTERNAL_DEST.", target ~ "." ) -%}
        {% endif %}
        {% if "dbt_internal_dest." in pred %}
            {%- set pred =  pred | replace("dbt_internal_dest.", target ~ "." ) -%}
        {% endif %}
        {% do predicates.append(pred) %}
    {% endfor %}

    {% if not model.batch or (not model.batch.event_time_start or not model.batch.event_time_end) -%}
        {% do exceptions.raise_compiler_error('dbt could not compute the start and end timestamps for the running batch') %}
    {% endif %}

    {#-- Add additional incremental_predicates to filter for batch --#}
    {% do predicates.append(model.config.event_time ~ " >= TIMESTAMP '" ~ model.batch.event_time_start ~ "'") %}
    {% do predicates.append(model.config.event_time ~ " < TIMESTAMP '" ~ model.batch.event_time_end ~ "'") %}
    {% do arg_dict.update({'incremental_predicates': predicates}) %}

    delete from {{ target }}
    where (
    {% for predicate in predicates %}
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


{% macro _update_predicates(target, incremental_predicates) %}

    {%- set predicates = [] -%}
    {% if incremental_predicates is not none %}
        {%- set incremental_predicates_list = [] + incremental_predicates -%}
        {%- for pred in incremental_predicates_list -%}
            {% if "DBT_INTERNAL_DEST." in pred %}
                {%- set pred =  pred | replace("DBT_INTERNAL_DEST.", target ~ "." ) -%}
            {% endif %}
            {% if "dbt_internal_dest." in pred %}
                {%- set pred =  pred | replace("dbt_internal_dest.", target ~ "." ) -%}
            {% endif %}
            {% do predicates.append(pred) %}
        {% endfor %}
    {% endif %}

    {%- do return(predicates) -%}

{% endmacro %}
