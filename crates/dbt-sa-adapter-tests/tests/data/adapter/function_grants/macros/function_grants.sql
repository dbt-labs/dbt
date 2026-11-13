{#- One grant per adapter, to a grantee that exists on every account. -#}
{% macro function_grant_config() %}
    {% if target.type == 'snowflake' %}
        {{ return({'usage': ['public']}) }}
    {% elif target.type == 'databricks' %}
        {{ return({'execute': ['account users']}) }}
    {% elif target.type == 'bigquery' %}
        {{ return({'roles/bigquery.dataViewer': ['special_group:allAuthenticatedUsers']}) }}
    {% else %}
        {{ return({'execute': ['public']}) }}
    {% endif %}
{% endmacro %}

{#- Apply the grants, then read them back from the warehouse. -#}
{% macro apply_and_show_function_grants(relation) %}
    {% if execute %}
        {% do log('function relation type: ' ~ relation.type, info=True) %}
        {% do apply_grants(relation, function_grant_config(), should_revoke=False) %}
        {% if target.type == 'snowflake' %}
            {% set grants = run_query(get_show_grant_sql(relation)) %}
            {% do log('function grants: ' ~ adapter.standardize_grants_dict(grants), info=True) %}
        {% elif target.type == 'databricks' %}
            {#- standardize_grants_dict keeps only ObjectType = TABLE rows, so read the raw rows. -#}
            {% set grants = run_query(get_show_grant_sql(relation)) %}
            {% for row in grants.rows if row['Principal'] == 'account users' and row['ObjectType'] == 'FUNCTION' %}
                {% do log('function grant: ' ~ row['ActionType'] ~ ' on ' ~ row['ObjectType'] ~ ' to ' ~ row['Principal'], info=True) %}
            {% endfor %}
        {% elif target.type == 'redshift' %}
            {% set grants = run_query(
                "select identity_name, privilege_type from svv_function_privileges"
                ~ " where namespace_name = '" ~ relation.schema ~ "'"
                ~ " and function_name = '" ~ relation.identifier ~ "'"
                ~ " and identity_name = 'public'"
            ) %}
            {% for row in grants.rows %}
                {% do log('function grant: ' ~ row['privilege_type'] ~ ' to ' ~ row['identity_name'], info=True) %}
            {% endfor %}
        {% endif %}
    {% endif %}
{% endmacro %}
