{% macro get_create_sql_refresh_schedule(refresh) %}
  {%- if refresh.cron -%}
    SCHEDULE CRON '{{ refresh.cron }}'{%- if refresh.time_zone_value %} AT TIME ZONE '{{ refresh.time_zone_value }}'{%- endif -%}
  {%- elif refresh.every -%}
    SCHEDULE EVERY {{ refresh.every }}
  {%- elif refresh.on_update -%}
    TRIGGER ON UPDATE{%- if refresh.at_most_every %} AT MOST EVERY INTERVAL {{ refresh.at_most_every }}{%- endif -%}
  {%- endif -%}
{% endmacro %}

{% macro get_alter_sql_refresh_schedule(refresh) %}
  {%- if not (refresh.cron or refresh.every or refresh.on_update) -%}
    DROP SCHEDULE
  {%- else -%}
    {{- 'ALTER ' if refresh.is_altered else 'ADD ' -}}{{- get_create_sql_refresh_schedule(refresh) -}}
  {%- endif -%}
{% endmacro %}
