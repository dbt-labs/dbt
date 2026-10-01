{% macro spark__copy_grants() %}

    {% if config.materialized == 'view' %}
        {#-- Spark views don't copy grants when they're replaced --#}
        {{ return(False) }}

    {% else %}
      {#-- This depends on how we're replacing the table, which depends on its file format
        -- Just play it safe by assuming that grants have been copied over, and need to be checked / possibly revoked
        -- We can make this more efficient in the future
      #}
        {{ return(True) }}

    {% endif %}
{% endmacro %}


{%- macro spark__get_grant_sql(relation, privilege, grantees) -%}
    grant {{ privilege }} on {{ relation }} to {{ adapter.quote(grantees[0]) }}
{%- endmacro %}


{%- macro spark__get_revoke_sql(relation, privilege, grantees) -%}
    revoke {{ privilege }} on {{ relation }} from {{ adapter.quote(grantees[0]) }}
{%- endmacro %}


{%- macro spark__support_multiple_grantees_per_dcl_statement() -%}
    {{ return(False) }}
{%- endmacro -%}


{% macro spark__call_dcl_statements(dcl_statement_list) %}
    {% for dcl_statement in dcl_statement_list %}
        {% call statement('grant_or_revoke') %}
            {{ dcl_statement }}
        {% endcall %}
    {% endfor %}
{% endmacro %}
