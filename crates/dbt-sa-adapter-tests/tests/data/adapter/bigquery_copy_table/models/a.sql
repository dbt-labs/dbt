{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema) %}

{% set source_non_empty = api.Relation.create(db_name, db_schema, "source_non_empty") %}

{% set non_existing_dest = api.Relation.create(db_name, db_schema, "non_existing_dest") %}
{% do adapter.copy_table(source_non_empty, non_existing_dest, "table") %} -- table materialization leads to write_truncate behavior
{% set _, table = adapter.execute("SELECT * FROM " ~ non_existing_dest, fetch=true) %}
{{ print("non_existing_dest + table (write_truncate) ") }}
{{ print(table.print_table(20, max_columns=6)) }}


{% set existing_dest = api.Relation.create(db_name, db_schema, "existing_dest") %}
{% set results = adapter.copy_table(source_non_empty, existing_dest, "incremental") %} -- incremental materialization leads to write_append behavior
{% set _, table = adapter.execute("SELECT * FROM " ~ existing_dest ~ " ORDER BY id", fetch=true) %}
{{ print("existing_dest + table (write_append) ") }}
{{ print(table.print_table(20, max_columns=6)) }}

SELECT 1 as one
