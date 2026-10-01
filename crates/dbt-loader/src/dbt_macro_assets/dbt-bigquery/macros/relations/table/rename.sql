-- funcsign: (relation, string) -> string
{%- macro bigquery__get_rename_table_sql(relation, new_name) -%}
    alter table {{ relation }} rename to {{ new_name }}
{%- endmacro -%}
