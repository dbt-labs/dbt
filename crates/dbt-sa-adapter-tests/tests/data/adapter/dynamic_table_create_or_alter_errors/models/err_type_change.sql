{% if var('evolved', false) %}select cast(id as varchar) as id, value{% else %}select id, value{% endif %} from {{ target.database }}.{{ target.schema }}.source_table
