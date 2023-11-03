SEED__MY_SEED = """
id,value
1,100
2,200
3,300
""".strip()


# postgres does not let you inspect the database for a last refresh date
# we need to create our own by adding a last_modified date to materialized view itself
MODEL__MY_MATERIALIZED_VIEW = """
{{ config(
    materialized="materialized_view"
) }}
select *, now() as last_refreshed from {{ ref('my_seed') }}
"""


# see above for why we are just querying the table instead of a metadata table
MACRO__LAST_REFRESH = """
{% macro postgres__test__last_refresh(schema, identifier) %}
    {% set _sql %}
        select max(last_refreshed) as last_refresh from {{ schema }}.{{ identifier }}
    {% endset %}
    {% do return(run_query(_sql)) %}
{% endmacro %}
"""
