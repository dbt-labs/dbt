{%- macro trino__get_create_materialized_view_as_sql(target_relation, sql) -%}
  create materialized view {{ target_relation }}
  {%- set grace_period = config.get('grace_period') %}
  {%- if grace_period is not none %}
    grace period {{ grace_period }}
  {%- endif %}
    {{ properties() }}
  as
  {{ sql }}
  ;
{%- endmacro -%}


{% macro trino__get_replace_materialized_view_as_sql(relation, sql, existing_relation, backup_relation, intermediate_relation) %}
    {{- trino__get_create_materialized_view_as_sql(intermediate_relation, sql) }}

    {% if existing_relation is not none %}
        {{ log("Found a " ~ existing_relation.type ~ " with same name. Will drop it", info=true) }}
        alter {{ existing_relation.type|replace("_", " ") }} {{ existing_relation }} rename to {{ backup_relation }};
    {% endif %}

    alter materialized view {{ intermediate_relation }} rename to {{ relation }};

{% endmacro %}


{#-- Applying materialized view configuration changes via alter is not supported. --#}
{#-- Return None, so `refresh_materialized_view` macro is invoked even --#}
{#-- if materialized view configuration changes are made. --#}
{#-- After configuration change, full refresh needs to be performed on mv. --#}
{% macro trino__get_materialized_view_configuration_changes(existing_relation, new_config) %}
    {% do return(None) %}
{% endmacro %}


{%- macro trino__refresh_materialized_view(relation) -%}
    refresh materialized view {{ relation }}
{%- endmacro -%}
