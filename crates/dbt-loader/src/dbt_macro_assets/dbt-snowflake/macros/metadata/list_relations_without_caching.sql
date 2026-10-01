{% macro snowflake__list_relations_without_caching(schema_relation, max_iter=10000, max_results_per_iter=10000) %}

    {%- if schema_relation is string -%}
        {%- set schema = schema_relation -%}
    {%- else -%}
        {%- set schema = schema_relation.include(identifier=False) -%}
    {%- endif -%}

    {%- set max_results_per_iter = adapter.config.flags.get('list_relations_per_page', max_results_per_iter) -%}
    {%- set max_iter = adapter.config.flags.get('list_relations_page_limit', max_iter) -%}
    {%- set too_many_relations_msg -%}
        dbt is currently configured to list a maximum of {{ max_results_per_iter * max_iter }} objects per schema.
        {{ schema }} exceeds this limit. If this is expected, you may configure this limit
        by setting list_relations_per_page and list_relations_page_limit in your project flags.
        It is recommended to start by increasing list_relations_page_limit.
    {%- endset -%}

    {%- set paginated_state = namespace(paginated_results=[], watermark=none) -%}

    {#-
        loop an extra time to catch the breach of max iterations
        Note: while range is 0-based, loop.index starts at 1
    -#}
    {%- for _ in range(max_iter + 1) -%}

        {#-
            raise a warning and break if we still didn't exit and we're beyond the max iterations limit
            Note: while range is 0-based, loop.index starts at 1
        -#}
        {%- if loop.index == max_iter + 1 -%}
            {%- do exceptions.warn(too_many_relations_msg) -%}
            {%- break -%}
        {%- endif -%}

        {%- set show_objects_sql = snowflake__show_objects_sql(schema, max_results_per_iter, paginated_state.watermark) -%}
        {%- set paginated_result = run_query(show_objects_sql) -%}
        {%- do paginated_state.paginated_results.append(paginated_result) -%}
        {%- set paginated_state.watermark = paginated_result.columns.get('name').values()[-1] -%}

        {#- we got less results than the max_results_per_iter (includes 0), meaning we reached the end -#}
        {%- if (paginated_result | length) < max_results_per_iter -%}
            {%- break -%}
        {%- endif -%}

    {%- endfor -%}

    {#- grab the first table in the paginated results to access the `merge` method -#}
    {%- set agate_table = paginated_state.paginated_results[0] -%}
    {%- do return(agate_table.merge(paginated_state.paginated_results)) -%}

{% endmacro %}


{% macro snowflake__show_objects_sql(schema, max_results_per_iter=10000, watermark=none) %}

{%- set _sql -%}
show objects in {{ schema }}
    limit {{ max_results_per_iter }}
    {% if watermark is not none -%} from '{{ watermark }}' {%- endif %}
;
{%- endset -%}

{%- do return(_sql) -%}

{% endmacro %}
