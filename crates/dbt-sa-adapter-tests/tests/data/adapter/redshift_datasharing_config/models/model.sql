{% set db_name = target.database %}

{% set verify_db = adapter.verify_database(db_name) %}
{{ print("verify_db using target.database: " ~  verify_db) }}

{% set verify_db = adapter.verify_database("another_db") %}
{{ print("verify_db using a non target.database: " ~ verify_db) }}

{% set datasharing_on = adapter.has_feature("datasharing") %}
{{ print("has_feature datasharing: " ~ datasharing_on) }}


select 1 as id
