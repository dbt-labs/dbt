{% macro singlestore__validate_unique_key_columns(unique_key, dest_columns) %}
    {%- set dest_col_names = dest_columns | map(attribute='name') | list -%}

    {%- if unique_key %}
        {%- if unique_key is sequence and unique_key is not string -%}
            {%- set keys = unique_key -%}
        {%- else -%}
            {%- set keys = [unique_key] -%}
        {%- endif -%}

        {%- for k in keys %}
            {%- if k not in dest_col_names %}
                {{ exceptions.raise_compiler_error(
                    "Incremental model unique_key '" ~ k ~ "' not found in model columns: "
                    ~ dest_col_names | join(', ')
                ) }}
            {%- endif %}
        {%- endfor %}
    {%- endif %}
{% endmacro %}

{% macro singlestore__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    {% if unique_key %}
        {% if unique_key is sequence and unique_key is not string %}
            delete {{ target.identifier }} from {{ target }}
            join {{ source }}
            where (
                {% for key in unique_key %}
                    {{ source }}.{{ key }} = {{ target }}.{{ key }}
                    {{ "and " if not loop.last }}
                {% endfor %}
                {% if incremental_predicates %}
                    {% for predicate in incremental_predicates %}
                        and {{ predicate }}
                    {% endfor %}
                {% endif %}
            );
        {% else %}
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
            {%- endif -%};
        {% endif %}
    {% endif %}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
    );
{% endmacro %}

{% macro singlestore__get_incremental_append_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set incremental_predicates = arg_dict.get("incremental_predicates", none) -%}
    {%- set dest_cols_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

    insert into {{ target }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ source }}
        {%- if incremental_predicates %}
        where {{ incremental_predicates | join(' and ') }}
        {%- endif %}
    );
{% endmacro %}

{% macro singlestore__get_incremental_delete_insert_sql(arg_dict) %}
    {% if arg_dict["unique_key"] %}
        {{ singlestore__validate_unique_key_columns(arg_dict["unique_key"], arg_dict["dest_columns"]) }}
    {% endif %}
    {{ singlestore__get_delete_insert_merge_sql(arg_dict["target_relation"], arg_dict["temp_relation"], arg_dict["unique_key"], arg_dict["dest_columns"], arg_dict["incremental_predicates"]) }}
{% endmacro %}

{% macro singlestore__get_incremental_merge_sql(arg_dict) %}
    {%- set target = arg_dict["target_relation"] -%}
    {%- set source = arg_dict["temp_relation"] -%}
    {%- set dest_columns = arg_dict["dest_columns"] -%}
    {%- set unique_key = arg_dict["unique_key"] -%}
    {%- set incremental_predicates = arg_dict.get("incremental_predicates", none) -%}
    {%- set col_names = dest_columns | map(attribute='name') | list -%}

    {%- if not unique_key -%}
        {{ singlestore__get_incremental_append_sql(arg_dict) }}
    {%- else -%}
        {{ singlestore__validate_unique_key_columns(unique_key, dest_columns) }}
        {%- if unique_key is sequence and unique_key is not string -%}
            {%- set unique_keys = unique_key -%}
        {%- else -%}
            {%- set unique_keys = [unique_key] -%}
        {%- endif -%}

        {%- set dest_cols_csv = get_quoted_csv(col_names) -%}

        {%- set update_assignments = [] -%}
        {%- for col in col_names -%}
            {%- if col not in unique_keys -%}
                {%- set qc = adapter.quote(col) -%}
                {%- do update_assignments.append(qc ~ ' = values(' ~ qc ~ ')') -%}
            {%- endif -%}
        {%- endfor -%}

        {%- if update_assignments | length > 0 -%}
            insert into {{ target }} ({{ dest_cols_csv }})
            select {{ dest_cols_csv }}
            from {{ source }}
            {%- if incremental_predicates %}
            where {{ incremental_predicates | join(' and ') }}
            {%- endif %}
            on duplicate key update
                {{ update_assignments | join(',\n        ') }};
        {%- else -%}
            insert ignore into {{ target }} ({{ dest_cols_csv }})
            select {{ dest_cols_csv }}
            from {{ source }}
            {%- if incremental_predicates %}
            where {{ incremental_predicates | join(' and ') }}
            {%- endif %};
        {%- endif -%}
    {%- endif -%}
{% endmacro %}

{% macro singlestore__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) %}
    {{ singlestore__get_incremental_merge_sql({'target_relation': target, 'temp_relation': source, 'unique_key': unique_key, 'dest_columns': dest_columns, 'incremental_predicates': incremental_predicates}) }}
{% endmacro %}

{% macro singlestore__get_incremental_default_sql(arg_dict) %}
    {% if arg_dict["unique_key"] %}
        {{ singlestore__get_incremental_delete_insert_sql(arg_dict) }}
    {% else %}
        {{ singlestore__get_incremental_append_sql(arg_dict) }}
    {% endif %}
{% endmacro %}
