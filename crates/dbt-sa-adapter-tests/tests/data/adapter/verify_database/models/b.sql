{% set db_name = target.database %}

-- expected to pass with postgres and redshift
{% set verify_db = adapter.verify_database(db_name) %}
{{ print("verify_db using target.database: " ~  verify_db) }}

-- expected to fail with postgres and redshift
-- postgres never allows cross db ref
-- redshift only allows this if ra3_node is set to true
{% set verify_db = adapter.verify_database("another_db") %}
{{ print("verify_db using a non target.database: " ~ verify_db) }}


select 1 as id