select id, value{% if var('evolved', false) %}, id * 10 as id_x10{% endif %} from {{ target.database }}.{{ target.schema }}.source_table
