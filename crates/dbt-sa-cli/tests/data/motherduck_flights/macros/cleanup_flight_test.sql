{% macro cleanup_flight_test() %}
  {% set flight_name = 'dbt-motherduck_flights-' ~ target.database ~ '-' ~ target.schema ~ '-flight_model' %}
  {% set flights = run_query(
      "select flight_id from MD_LIST_FLIGHTS(\"limit\" := 200, owner_only := true)"
      ~ " where flight_name = '" ~ flight_name | replace("'", "''") ~ "'"
  ) %}

  {% if execute %}
    {% for row in flights.rows %}
      {% do run_query(
          "select * from MD_DELETE_FLIGHT(flight_id := '"
          ~ row[0] | replace("'", "''") ~ "')"
      ) %}
    {% endfor %}
  {% endif %}

  {% do run_query(
      "drop schema if exists "
      ~ adapter.quote(target.database) ~ "." ~ adapter.quote(target.schema)
      ~ " cascade"
  ) %}
{% endmacro %}
