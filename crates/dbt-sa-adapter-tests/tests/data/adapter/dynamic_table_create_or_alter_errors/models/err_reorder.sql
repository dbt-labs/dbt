{% if var('evolved', false) %}select value, id{% else %}select id, value{% endif %} from {{ target.database }}.{{ target.schema }}.source_table
