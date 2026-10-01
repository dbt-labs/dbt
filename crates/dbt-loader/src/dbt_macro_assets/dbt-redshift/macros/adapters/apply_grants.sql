{#-- Normalize a grants config dict by adding 'user:' prefix to unprefixed entries --#}
{% macro redshift__normalize_grants_dict(grants_dict) %}
    {%- set normalized = {} -%}
    {%- for privilege, grantees in grants_dict.items() -%}
        {%- set normalized_grantees = [] -%}
        {%- for grantee in grantees -%}
            {%- if grantee.startswith('group:') or grantee.startswith('role:') or grantee.startswith('user:') -%}
                {%- do normalized_grantees.append(grantee) -%}
            {%- else -%}
                {#-- Unprefixed = user, add prefix for consistent comparison --#}
                {%- do normalized_grantees.append('user:' ~ grantee) -%}
            {%- endif -%}
        {%- endfor -%}
        {%- do normalized.update({privilege: normalized_grantees}) -%}
    {%- endfor -%}
    {{ return(normalized) }}
{% endmacro %}


{#-- Override apply_grants to normalize config before delegating to default --#}
{% macro redshift__apply_grants(relation, grant_config, should_revoke=True) %}
    {% if grant_config %}
        {% if redshift__use_grants_extended() %}
            {% set normalized_grant_config = redshift__normalize_grants_dict(grant_config) %}
            {{ default__apply_grants(relation, normalized_grant_config, should_revoke) }}
        {% else %}
            {{ default__apply_grants(relation, grant_config, should_revoke) }}
        {% endif %}
    {% endif %}
{% endmacro %}


{% macro redshift__get_show_grant_sql(relation) %}
  {% if redshift__use_show_apis() %}
{#-
    Use SHOW GRANTS for cross-database support (required for RA3/datasharing).
    Note: SHOW GRANTS conflates groups and roles — groups appear with a '/'
    prefix on identity_name and identity_type='role'. The standardize_grants_dict
    method in RedshiftAdapter handles this translation.
-#}
    SHOW GRANTS ON TABLE {{ adapter.quote(relation.database) }}.{{ adapter.quote(relation.schema) }}.{{ adapter.quote(relation.identifier) }}
  {% elif redshift__use_grants_extended() %}
{#-
    Use svv_relation_privileges for same-database grants. This view correctly
    distinguishes users, groups, and roles via the identity_type column.
    Requires superuser or SYSLOG ACCESS UNRESTRICTED for full visibility.
    Does NOT support cross-database references.
-#}
    select
        identity_name,
        identity_type,
        privilege_type
    from svv_relation_privileges
    where namespace_name = '{{ relation.schema }}'
      and relation_name = '{{ relation.identifier }}'
      and not (identity_type = 'user' and identity_name = current_user)
  {% else %}
{#-
    Legacy path: query pg_user with has_table_privilege(). Only detects user
    grants; groups and roles are not visible. Returns a 'grantee' column
    compatible with the base standardize_grants_dict implementation.
-#}
    with privileges as (
         -- valid options per https://docs.aws.amazon.com/redshift/latest/dg/r_HAS_TABLE_PRIVILEGE.html
        select 'select' as privilege_type
        union all
        select 'insert' as privilege_type
        union all
        select 'update' as privilege_type
        union all
        select 'delete' as privilege_type
        union all
        select 'references' as privilege_type
    )
    select
        u.usename as grantee,
        p.privilege_type
    from pg_user u
    cross join privileges p
    where has_table_privilege(u.usename, '{{ relation }}', privilege_type)
        and u.usename != current_user
        and not u.usesuper
  {% endif %}
{% endmacro %}


{#-- Format prefixed grantees into Redshift SQL syntax --#}
{% macro redshift__format_grantees(grantees) %}
    {%- set formatted = [] -%}
    {%- for grantee in grantees -%}
        {%- if grantee.startswith('group:') -%}
            {%- do formatted.append('GROUP ' ~ grantee[6:]) -%}
        {%- elif grantee.startswith('role:') -%}
            {%- do formatted.append('ROLE ' ~ grantee[5:]) -%}
        {%- elif grantee.startswith('user:') -%}
            {%- do formatted.append(grantee[5:]) -%}
        {%- else -%}
            {%- do formatted.append(grantee) -%}
        {%- endif -%}
    {%- endfor -%}
    {{ return(formatted) }}
{% endmacro %}


{% macro redshift__get_grant_sql(relation, privilege, grantees) %}
    {% if redshift__use_grants_extended() %}
        {%- set formatted_grantees = redshift__format_grantees(grantees) -%}
        grant {{ privilege }} on {{ relation.render() }} to {{ formatted_grantees | join(', ') }}
    {% else %}
        {{ default__get_grant_sql(relation, privilege, grantees) }}
    {% endif %}
{% endmacro %}

{% macro redshift__get_revoke_sql(relation, privilege, grantees) %}
    {% if redshift__use_grants_extended() %}
        {%- set formatted_grantees = redshift__format_grantees(grantees) -%}
        revoke {{ privilege }} on {{ relation.render() }} from {{ formatted_grantees | join(', ') }}
    {% else %}
        {{ default__get_revoke_sql(relation, privilege, grantees) }}
    {% endif %}
{% endmacro %}
