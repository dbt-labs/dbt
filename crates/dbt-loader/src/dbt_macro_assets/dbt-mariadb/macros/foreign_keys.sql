{#- Foreign key names are unique per schema in MariaDB, so the intermediate
    `__dbt_tmp` table cannot reuse the names the model gives its foreign keys
    while the existing table still holds them. The intermediate table gets
    generated names (`<table>_ibfk_N`, which follow RENAME TABLE), and the
    given names come back during the swap:
      1. existing -> backup: drop the named foreign keys of the existing table,
         which is dropped right after;
      2. intermediate -> target: rename the generated foreign keys to the
         given names. -#}

{% macro mariadb__is_model_target(relation) -%}
  {{ return(this is defined and this is not none
            and relation.schema | lower == this.schema | lower
            and relation.identifier | lower == this.identifier | lower) }}
{%- endmacro %}

{% macro mariadb__named_foreign_keys() -%}
  {%- set named = [] -%}
  {%- if model is defined and model is mapping -%}
    {%- for constraint in model.get('constraints') or [] -%}
      {%- if constraint['type'] == 'foreign_key' and constraint['name'] -%}
        {%- do named.append(constraint) -%}
      {%- endif -%}
    {%- endfor -%}
  {%- endif -%}
  {{ return(named) }}
{%- endmacro %}

{#- Like `table_columns_and_constraints`, optionally without foreign key names. -#}
{% macro mariadb__table_columns_and_constraints(unnamed_foreign_keys=False) -%}
  {%- set model_constraints = [] -%}
  {%- for constraint in model['constraints'] -%}
    {%- if unnamed_foreign_keys and constraint['type'] == 'foreign_key' -%}
      {%- set unnamed = {} -%}
      {%- do unnamed.update(constraint) -%}
      {%- do unnamed.update({'name': none}) -%}
      {%- do model_constraints.append(unnamed) -%}
    {%- else -%}
      {%- do model_constraints.append(constraint) -%}
    {%- endif -%}
  {%- endfor -%}
  {%- set raw_column_constraints = adapter.render_raw_columns_constraints(raw_columns=model['columns']) -%}
  {%- set raw_model_constraints = adapter.render_raw_model_constraints(raw_constraints=model_constraints) -%}
    (
    {% for c in raw_column_constraints -%}
      {{ c }}{{ "," if not loop.last or raw_model_constraints }}
    {% endfor %}
    {% for c in raw_model_constraints -%}
        {{ c }}{{ "," if not loop.last }}
    {% endfor -%}
    )
{%- endmacro %}

{% macro mariadb__foreign_keys(relation) -%}
  {% call statement('foreign_keys', fetch_result=True, auto_begin=False) -%}
    select
      k.constraint_name,
      group_concat(k.column_name order by k.ordinal_position separator ',') as column_names,
      k.referenced_table_schema,
      k.referenced_table_name,
      group_concat(k.referenced_column_name order by k.ordinal_position separator ',') as referenced_column_names,
      r.delete_rule,
      r.update_rule
    from information_schema.key_column_usage as k
    join information_schema.referential_constraints as r
      on r.constraint_schema = k.constraint_schema
      and r.table_name = k.table_name
      and r.constraint_name = k.constraint_name
    where k.table_schema = '{{ relation.schema | replace("\\", "\\\\") | replace("'", "''") }}'
      and k.table_name = '{{ relation.identifier | replace("\\", "\\\\") | replace("'", "''") }}'
      and k.referenced_table_name is not null
    group by k.constraint_name, k.referenced_table_schema, k.referenced_table_name, r.delete_rule, r.update_rule
  {%- endcall %}
  {{ return(load_result('foreign_keys').table.rows) }}
{%- endmacro %}

{% macro mariadb__drop_named_foreign_keys(relation, named_foreign_keys) -%}
  {%- set names = named_foreign_keys | map(attribute='name') | map('lower') | list -%}
  {%- for row in mariadb__foreign_keys(relation) if row[0] | lower in names -%}
    {% call statement('drop_named_foreign_key', auto_begin=False) -%}
      alter table {{ relation }} drop foreign key {{ adapter.quote(row[0]) }}
    {%- endcall %}
  {%- endfor -%}
{%- endmacro %}

{% macro mariadb__restore_foreign_key_names(relation, named_foreign_keys) -%}
  {%- set rows = mariadb__foreign_keys(relation) -%}
  {%- set existing = rows | map(attribute=0) | map('lower') | list -%}
  {%- set names = named_foreign_keys | map(attribute='name') | map('lower') | list -%}
  {%- set renamed = [] -%}
  {%- for constraint in named_foreign_keys if constraint['name'] | lower not in existing -%}
    {%- set columns = constraint['columns'] | map('replace', '`', '') | join(',') | lower -%}
    {%- for row in rows if row[0] | lower not in names and row[0] not in renamed
                          and row[1] | lower == columns -%}
      {%- if loop.first -%}
        {%- do renamed.append(row[0]) -%}
        {#- Drop and add in one statement; the rows were just validated, so
            skipping the foreign key checks keeps it in place, without a copy. -#}
        {% call statement('restore_foreign_key_name', auto_begin=False) -%}
          set statement foreign_key_checks = 0 for
          alter table {{ relation }}
            drop foreign key {{ adapter.quote(row[0]) }},
            add constraint {{ adapter.quote(constraint['name']) }}
              foreign key ({% for c in row[1].split(',') %}{{ adapter.quote(c) }}{{ ", " if not loop.last }}{% endfor %})
              references {{ adapter.quote(row[2]) }}.{{ adapter.quote(row[3]) }}
                ({% for c in row[4].split(',') %}{{ adapter.quote(c) }}{{ ", " if not loop.last }}{% endfor %})
              on delete {{ row[5] }} on update {{ row[6] }}
        {%- endcall %}
      {%- endif -%}
    {%- endfor -%}
  {%- endfor -%}
{%- endmacro %}
