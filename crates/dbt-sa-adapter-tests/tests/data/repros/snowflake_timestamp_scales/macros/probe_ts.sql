{# The test harness masks every `dd:dd:dd` in dbt's output as `HH:MM:SS`, so
   values are logged with `_` in place of `:`. #}
{% macro unmasked(v) -%}
  {{ v | string | replace(':', '_') }}
{%- endmacro %}

{# Logs what run_query returns for TIMESTAMP_NTZ/LTZ/TZ of every scale, next to
   Snowflake's own rendering of the NTZ value. #}
{% macro probe_ts() %}
  {% set lit = "'2026-11-16 10:49:35.861123456 +05:30'" %}
  {% for s in range(0, 10) %}
    {% set q %}
      select
        {{ lit }}::timestamp_ntz({{ s }}) as ntz,
        {{ lit }}::timestamp_ltz({{ s }}) as ltz,
        {{ lit }}::timestamp_tz({{ s }}) as tz,
        to_varchar({{ lit }}::timestamp_ntz({{ s }}), 'YYYY-MM-DD HH24:MI:SS.FF9') as ntz_varchar,
        date_part(epoch_nanosecond, {{ lit }}::timestamp_ntz({{ s }})) as ntz_epoch_ns
    {% endset %}
    {% set r = run_query(q) %}
    {{ log('scale ' ~ s
        ~ ' | ntz=' ~ unmasked(r.columns[0].values()[0])
        ~ ' | ltz=' ~ unmasked(r.columns[1].values()[0])
        ~ ' | tz=' ~ unmasked(r.columns[2].values()[0])
        ~ ' | ntz_varchar=' ~ unmasked(r.columns[3].values()[0])
        ~ ' | ntz_epoch_ns=' ~ r.columns[4].values()[0], info=True) }}
  {% endfor %}

  {# The reporter's watermark pattern: max() over a scale 7 column of a built table. #}
  {% set w = run_query('select max(ntz_7) from ' ~ ref('ts_scales')) %}
  {{ log('watermark (scale 7) = ' ~ unmasked(w.columns[0].values()[0]), info=True) }}
{% endmacro %}

{# Logs what run_query returns for TIME of every scale. #}
{% macro probe_time() %}
  {% for s in range(0, 10) %}
    {% set q %}
      select
        '10:49:35.861123456'::time({{ s }}) as t,
        to_varchar('10:49:35.861123456'::time({{ s }}), 'HH24:MI:SS.FF9') as t_varchar
    {% endset %}
    {% set r = run_query(q) %}
    {{ log('scale ' ~ s
        ~ ' | time=' ~ unmasked(r.columns[0].values()[0])
        ~ ' | time_varchar=' ~ unmasked(r.columns[1].values()[0]), info=True) }}
  {% endfor %}
{% endmacro %}
