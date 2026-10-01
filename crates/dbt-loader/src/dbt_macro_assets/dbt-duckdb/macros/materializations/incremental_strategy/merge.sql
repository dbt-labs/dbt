{% macro duckdb__merge_join_clause(unique_key, merge_on_using_columns, incremental_predicates) -%}
    {%- set incremental_filters = [] if incremental_predicates is none else [] + incremental_predicates -%}
    {%- set join_predicates = [] -%}
    {%- set using_clause_requested = merge_on_using_columns and merge_on_using_columns | length > 0 -%}

    {%- if not using_clause_requested -%}
        {% if unique_key %}
            {% if unique_key is sequence and unique_key is not mapping and unique_key is not string %}
                {% for key in unique_key %}
                    {% do join_predicates.append("DBT_INTERNAL_SOURCE." ~ key ~ " = DBT_INTERNAL_DEST." ~ key) %}
                {% endfor %}
            {% else %}
                {% do join_predicates.append("DBT_INTERNAL_SOURCE." ~ unique_key ~ " = DBT_INTERNAL_DEST." ~ unique_key) %}
            {% endif %}
        {% else %}
            {% do join_predicates.append('FALSE') %}
        {% endif %}
    {%- endif -%}

    {% if using_clause_requested %}
        USING ({{ merge_on_using_columns | join(', ') }})
        {%- if incremental_filters | length > 0 %}
        ON ({{ incremental_filters | join(") AND (") }})
        {%- endif %}
    {% else %}
        ON ({{ (join_predicates + incremental_filters) | join(") AND (") }})
    {% endif %}
{%- endmacro %}

{% macro duckdb__get_incremental_merge_sql(args_dict) %}
  {%- set target_relation = args_dict['target_relation'] -%}
  {%- set temp_relation = args_dict['temp_relation'] -%}
  {%- set unique_key = args_dict['unique_key'] -%}
  {%- set dest_columns = args_dict['dest_columns'] -%}
  {%- set incremental_predicates = normalize_incremental_predicates(args_dict.get('incremental_predicates')) -%}

  {%- set build_sql = duckdb__get_merge_sql(target_relation, temp_relation, unique_key, dest_columns, incremental_predicates) -%}

  {{ return(build_sql) }}
{% endmacro %}

{% macro duckdb__get_merge_sql(target, source, unique_key, dest_columns, incremental_predicates=none) -%}
    {%- set predicates = normalize_incremental_predicates(incremental_predicates) -%}

    {{ validate_merge_config(config, target) }}

    {%- set sql_header = config.get('sql_header', none) -%}
    {%- set merge_on_using_columns = config.get('merge_on_using_columns', []) -%}
    {%- set merge_update_condition = config.get('merge_update_condition', none) -%}
    {%- set merge_insert_condition = config.get('merge_insert_condition', none) -%}
    {%- set merge_update_columns = config.get('merge_update_columns', []) -%}
    {%- set merge_exclude_columns = config.get('merge_exclude_columns', []) -%}
    {%- set merge_update_set_expressions = config.get('merge_update_set_expressions', {}) -%}
    {%- set merge_returning_columns = config.get('merge_returning_columns', []) -%}

    {%- set merge_clause_defaults = merge_clause_defaults(
        merge_update_condition,
        merge_insert_condition,
        merge_update_columns,
        merge_exclude_columns,
        merge_update_set_expressions
    ) -%}

    {%- set merge_clauses = config.get('merge_clauses', {}) -%}
    {%- set when_matched_clauses = merge_clauses.get('when_matched', [
        merge_clause_defaults.when_matched_update_explicit
        if (merge_update_columns|length != 0 or merge_exclude_columns|length != 0 or merge_update_set_expressions|length != 0)
        else merge_clause_defaults.when_matched_update_by_name
    ]) -%}

    {%- set when_not_matched_clauses = merge_clauses.get('when_not_matched', [
        merge_clause_defaults.when_not_matched_insert_by_name
    ]) -%}

    {{ sql_header if sql_header is not none }}

    MERGE INTO {{ target }} AS DBT_INTERNAL_DEST
        USING {{ source }} AS DBT_INTERNAL_SOURCE
        {{ duckdb__merge_join_clause(unique_key, merge_on_using_columns, incremental_predicates) }}

    {%- for when_matched in when_matched_clauses %}
    WHEN MATCHED
        {%- if when_matched.get('condition') -%}
            {%- if when_matched.get('condition') is string %} AND {{ when_matched.get('condition') }}
            {%- else %} AND ({{ when_matched.get('condition') | join(') AND (') }})
            {%- endif -%}
        {%- endif %}
    THEN
        {% if when_matched.get('action') == 'update' %}
            {%- if when_matched.get('mode') == 'by_name' -%}
            UPDATE BY NAME
            {%- elif when_matched.get('mode') == 'by_position' -%}
            UPDATE BY POSITION
            {%- elif when_matched.get('mode') == 'star' -%}
            UPDATE SET *
            {%- elif when_matched.get('mode') == 'explicit' -%}
                {%- set include_columns = when_matched.get('update', {}).get('include', []) -%}
                {%- set exclude_columns = when_matched.get('update', {}).get('exclude', []) -%}
                {%- set set_expressions = when_matched.get('update', {}).get('set_expressions', {}) -%}

                {%- set update_columns = get_merge_update_columns(include_columns, exclude_columns, dest_columns) -%}

                {%- set update_columns_after_overrides = [] -%}
                {%- for column in update_columns -%}
                    {%- set unquoted_column = column.replace('"', '').replace("'", "") -%}
                    {%- if unquoted_column not in set_expressions -%}
                    {%- do update_columns_after_overrides.append(column) -%}
                    {%- endif -%}
                {%- endfor -%}

            UPDATE SET
                {% for column, expression in set_expressions.items() -%}
                {{ column }} = {{ expression }}{% if not loop.last or update_columns_after_overrides|length > 0 %}, {% endif %}
                {%- endfor %}
                {%- for column in update_columns_after_overrides -%}
                {{ column }} = DBT_INTERNAL_SOURCE.{{ column }}{% if not loop.last %}, {% endif %}
                {%- endfor %}
            {%- endif -%}

        {%- elif when_matched.get('action') == 'delete' %}
            DELETE
        {%- elif when_matched.get('action') == 'do_nothing' %}
            DO NOTHING
        {%- elif when_matched.get('action') == 'error' %}
            {%- set error_message = (when_matched.get('error_message', '') or '') | replace("'", "''") -%}
            ERROR{% if when_matched.get('error_message') %} '{{ error_message }}'{% endif %}
        {%- endif %}
    {%- endfor %}

    {%- for when_not_matched in when_not_matched_clauses %}
    WHEN NOT MATCHED
        {% if when_not_matched.get('by') %}BY {{ when_not_matched.get('by') | upper }} {% endif %}
        {%- if when_not_matched.get('condition') -%}
            {%- if when_not_matched.get('condition') is string %} AND {{ when_not_matched.get('condition') }}
            {%- else %} AND ({{ when_not_matched.get('condition') | join(') AND (') }})
            {%- endif -%}
        {%- endif %}
    THEN
        {% if when_not_matched.get('action') == 'update' %}
            {%- set set_expressions = when_not_matched.get('set_expressions', {}) -%}

            UPDATE SET
            {% for column, expression in set_expressions.items() -%}
            {{ column }} = {{ expression }}{% if not loop.last %}, {% endif %}
            {%- endfor %}

        {%- elif when_not_matched.get('action') == 'insert' %}
            {%- if when_not_matched.get('mode') == 'by_name' -%}
            INSERT BY NAME
            {%- elif when_not_matched.get('mode') == 'by_position' -%}
            INSERT BY POSITION
            {%- elif when_not_matched.get('mode') == 'star' -%}
            INSERT *
            {%- elif when_not_matched.get('mode') == 'explicit' -%}
            {%- set insert_columns = when_not_matched.get('insert', {}).get('columns', []) -%}
            {%- set insert_values = when_not_matched.get('insert', {}).get('values', []) -%}

            INSERT
                ({{ insert_columns | join(', ') }})
                VALUES ({{ insert_values  | join(', ') }})
            {%- endif -%}
        {%- elif when_not_matched.get('action') == 'delete' %}
            DELETE
        {%- elif when_not_matched.get('action') == 'do_nothing' %}
            DO NOTHING
        {%- elif when_not_matched.get('action') == 'error' %}
            {%- set error_message = (when_not_matched.get('error_message', '') or '') | replace("'", "''") -%}
            ERROR{% if when_not_matched.get('error_message') %} '{{ error_message }}'{% endif %}
        {%- endif %}
    {%- endfor %}

    {%- if merge_returning_columns %}
    RETURNING {{ merge_returning_columns if merge_returning_columns is string else merge_returning_columns | join(', ') }}
    {%- endif %}
{% endmacro %}
