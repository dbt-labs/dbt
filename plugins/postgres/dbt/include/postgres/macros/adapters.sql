{% macro postgres__create_table_as(temporary, relation, sql) -%}
  {%- set unlogged = config.get('unlogged', default=false) -%}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}

  create {% if temporary -%}
    temporary
  {%- elif unlogged -%}
    unlogged
  {%- endif %} table {{ relation }}
  {% set contract_config = config.get('contract') %}
  {% if contract_config.enforced %}
    {{ get_assert_columns_equivalent(sql) }}
  {% endif -%}
  {% if contract_config.enforced and (not temporary) -%}
      {{ get_table_columns_and_constraints() }} ;
    insert into {{ relation }} (
      {{ adapter.dispatch('get_column_names', 'dbt')() }}
    )
    {%- set sql = get_select_subquery(sql) %}
  {% else %}
    as
  {% endif %}
  (
    {{ sql }}
  );
{%- endmacro %}

{% macro postgres__get_create_index_sql(relation, index_dict) -%}
  {%- set index_config = adapter.parse_index(index_dict) -%}
  {%- set comma_separated_columns = ", ".join(index_config.columns) -%}
  {%- set index_name = index_config.render(relation) -%}

  create {% if index_config.unique -%}
    unique
  {%- endif %} index if not exists
  "{{ index_name }}"
  on {{ relation }} {% if index_config.type -%}
    using {{ index_config.type }}
  {%- endif %}
  ({{ comma_separated_columns }});
{%- endmacro %}

{% macro postgres__create_schema(relation) -%}
  {% if relation.database -%}
    {{ adapter.verify_database(relation.database) }}
  {%- endif -%}
  {%- call statement('create_schema') -%}
    create schema if not exists {{ relation.without_identifier().include(database=False) }}
  {%- endcall -%}
{% endmacro %}

{% macro postgres__drop_schema(relation) -%}
  {% if relation.database -%}
    {{ adapter.verify_database(relation.database) }}
  {%- endif -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier().include(database=False) }} cascade
  {%- endcall -%}
{% endmacro %}

{% macro postgres__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}

      with information_schema_columns as (

 SELECT (current_database())::information_schema.sql_identifier AS table_catalog,
    (nc.nspname)::information_schema.sql_identifier AS table_schema,
    (c.relname)::information_schema.sql_identifier AS table_name,
    (a.attname)::information_schema.sql_identifier AS column_name,
    (a.attnum)::information_schema.cardinal_number AS ordinal_position,
    (
        CASE
            WHEN (a.attgenerated = ''::"char") THEN pg_get_expr(ad.adbin, ad.adrelid)
            ELSE NULL::text
        END)::information_schema.character_data AS column_default,
    (
        CASE
            WHEN (a.attnotnull OR ((t.typtype = 'd'::"char") AND t.typnotnull)) THEN 'NO'::text
            ELSE 'YES'::text
        END)::information_schema.yes_or_no AS is_nullable,
    (
        CASE
            WHEN (t.typtype = 'd'::"char") THEN
            CASE
                WHEN ((bt.typelem <> (0)::oid) AND (bt.typlen = '-1'::integer)) THEN 'ARRAY'::text
                WHEN (nbt.nspname = 'pg_catalog'::name) THEN format_type(t.typbasetype, NULL::integer)
                ELSE 'USER-DEFINED'::text
            END
            ELSE
            CASE
                WHEN ((t.typelem <> (0)::oid) AND (t.typlen = '-1'::integer)) THEN 'ARRAY'::text
                WHEN (nt.nspname = 'pg_catalog'::name) THEN format_type(a.atttypid, NULL::integer)
                ELSE 'USER-DEFINED'::text
            END
        END)::information_schema.character_data AS data_type,
    (information_schema._pg_char_max_length(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS character_maximum_length,
    (information_schema._pg_char_octet_length(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS character_octet_length,
    (information_schema._pg_numeric_precision(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS numeric_precision,
    (information_schema._pg_numeric_precision_radix(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS numeric_precision_radix,
    (information_schema._pg_numeric_scale(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS numeric_scale,
    (information_schema._pg_datetime_precision(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.cardinal_number AS datetime_precision,
    (information_schema._pg_interval_type(information_schema._pg_truetypid(a.*, t.*), information_schema._pg_truetypmod(a.*, t.*)))::information_schema.character_data AS interval_type,
    (NULL::integer)::information_schema.cardinal_number AS interval_precision,
    (NULL::name)::information_schema.sql_identifier AS character_set_catalog,
    (NULL::name)::information_schema.sql_identifier AS character_set_schema,
    (NULL::name)::information_schema.sql_identifier AS character_set_name,
    (
        CASE
            WHEN (nco.nspname IS NOT NULL) THEN current_database()
            ELSE NULL::name
        END)::information_schema.sql_identifier AS collation_catalog,
    (nco.nspname)::information_schema.sql_identifier AS collation_schema,
    (co.collname)::information_schema.sql_identifier AS collation_name,
    (
        CASE
            WHEN (t.typtype = 'd'::"char") THEN current_database()
            ELSE NULL::name
        END)::information_schema.sql_identifier AS domain_catalog,
    (
        CASE
            WHEN (t.typtype = 'd'::"char") THEN nt.nspname
            ELSE NULL::name
        END)::information_schema.sql_identifier AS domain_schema,
    (
        CASE
            WHEN (t.typtype = 'd'::"char") THEN t.typname
            ELSE NULL::name
        END)::information_schema.sql_identifier AS domain_name,
    (current_database())::information_schema.sql_identifier AS udt_catalog,
    (COALESCE(nbt.nspname, nt.nspname))::information_schema.sql_identifier AS udt_schema,
    (COALESCE(bt.typname, t.typname))::information_schema.sql_identifier AS udt_name,
    (NULL::name)::information_schema.sql_identifier AS scope_catalog,
    (NULL::name)::information_schema.sql_identifier AS scope_schema,
    (NULL::name)::information_schema.sql_identifier AS scope_name,
    (NULL::integer)::information_schema.cardinal_number AS maximum_cardinality,
    (a.attnum)::information_schema.sql_identifier AS dtd_identifier,
    ('NO'::character varying)::information_schema.yes_or_no AS is_self_referencing,
    (
        CASE
            WHEN (a.attidentity = ANY (ARRAY['a'::"char", 'd'::"char"])) THEN 'YES'::text
            ELSE 'NO'::text
        END)::information_schema.yes_or_no AS is_identity,
    (
        CASE a.attidentity
            WHEN 'a'::"char" THEN 'ALWAYS'::text
            WHEN 'd'::"char" THEN 'BY DEFAULT'::text
            ELSE NULL::text
        END)::information_schema.character_data AS identity_generation,
    (seq.seqstart)::information_schema.character_data AS identity_start,
    (seq.seqincrement)::information_schema.character_data AS identity_increment,
    (seq.seqmax)::information_schema.character_data AS identity_maximum,
    (seq.seqmin)::information_schema.character_data AS identity_minimum,
    (
        CASE
            WHEN seq.seqcycle THEN 'YES'::text
            ELSE 'NO'::text
        END)::information_schema.yes_or_no AS identity_cycle,
    (
        CASE
            WHEN (a.attgenerated <> ''::"char") THEN 'ALWAYS'::text
            ELSE 'NEVER'::text
        END)::information_schema.character_data AS is_generated,
    (
        CASE
            WHEN (a.attgenerated <> ''::"char") THEN pg_get_expr(ad.adbin, ad.adrelid)
            ELSE NULL::text
        END)::information_schema.character_data AS generation_expression,
    (
        CASE
            WHEN ((c.relkind = ANY (ARRAY['r'::"char", 'p'::"char"])) OR ((c.relkind = ANY (ARRAY['v'::"char", 'f'::"char"])) AND pg_column_is_updatable((c.oid)::regclass, a.attnum, false))) THEN 'YES'::text
            ELSE 'NO'::text
        END)::information_schema.yes_or_no AS is_updatable
   FROM ((((((pg_attribute a
     LEFT JOIN pg_attrdef ad ON (((a.attrelid = ad.adrelid) AND (a.attnum = ad.adnum))))
     JOIN (pg_class c
     JOIN pg_namespace nc ON ((c.relnamespace = nc.oid))) ON ((a.attrelid = c.oid)))
     JOIN (pg_type t
     JOIN pg_namespace nt ON ((t.typnamespace = nt.oid))) ON ((a.atttypid = t.oid)))
     LEFT JOIN (pg_type bt
     JOIN pg_namespace nbt ON ((bt.typnamespace = nbt.oid))) ON (((t.typtype = 'd'::"char") AND (t.typbasetype = bt.oid))))
     LEFT JOIN (pg_collation co
     JOIN pg_namespace nco ON ((co.collnamespace = nco.oid))) ON (((a.attcollation = co.oid) AND ((nco.nspname <> 'pg_catalog'::name) OR (co.collname <> 'default'::name)))))
     LEFT JOIN (pg_depend dep
     JOIN pg_sequence seq ON (((dep.classid = ('pg_class'::regclass)::oid) AND (dep.objid = seq.seqrelid) AND (dep.deptype = 'i'::"char")))) ON (((dep.refclassid = ('pg_class'::regclass)::oid) AND (dep.refobjid = c.oid) AND (dep.refobjsubid = a.attnum))))
  WHERE ((NOT pg_is_other_temp_schema(nc.oid)) AND (a.attnum > 0) AND (NOT a.attisdropped) AND (c.relkind = ANY (ARRAY['r'::"char", 'v'::"char", 'f'::"char", 'p'::"char", 'm'::"char"])) AND (pg_has_role(c.relowner, 'USAGE'::text) OR has_column_privilege(c.oid, a.attnum, 'SELECT, INSERT, UPDATE, REFERENCES'::text)))

      )

      select
          column_name,
          data_type,
          character_maximum_length,
          numeric_precision,
          numeric_scale

      from information_schema_columns
      where table_name = '{{ relation.identifier }}'
        {% if relation.schema %}
        and table_schema = '{{ relation.schema }}'
        {% endif %}
      order by ordinal_position

  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{% endmacro %}


{% macro postgres__list_relations_without_caching(schema_relation) %}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      '{{ schema_relation.database }}' as database,
      tablename as name,
      schemaname as schema,
      'table' as type
    from pg_tables
    where schemaname ilike '{{ schema_relation.schema }}'
    union all
    select
      '{{ schema_relation.database }}' as database,
      viewname as name,
      schemaname as schema,
      'view' as type
    from pg_views
    where schemaname ilike '{{ schema_relation.schema }}'
    union all
    select
      '{{ schema_relation.database }}' as database,
      matviewname as name,
      schemaname as schema,
      'materialized_view' as type
    from pg_matviews
    where schemaname ilike '{{ schema_relation.schema }}'
  {% endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{% endmacro %}

{% macro postgres__information_schema_name(database) -%}
  {% if database_name -%}
    {{ adapter.verify_database(database_name) }}
  {%- endif -%}
  information_schema
{%- endmacro %}

{% macro postgres__list_schemas(database) %}
  {% if database -%}
    {{ adapter.verify_database(database) }}
  {%- endif -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) %}
    select distinct nspname from pg_namespace
  {% endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}

{% macro postgres__check_schema_exists(information_schema, schema) -%}
  {% if information_schema.database -%}
    {{ adapter.verify_database(information_schema.database) }}
  {%- endif -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) %}
    select count(*) from pg_namespace where nspname = '{{ schema }}'
  {% endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}

{#
  Postgres tables have a maximum length of 63 characters, anything longer is silently truncated.
  Temp and backup relations add a lot of extra characters to the end of table names to ensure uniqueness.
  To prevent this going over the character limit, the base_relation name is truncated to ensure
  that name + suffix + uniquestring is < 63 characters.
#}

{% macro postgres__make_relation_with_suffix(base_relation, suffix, dstring) %}
    {% if dstring %}
      {% set dt = modules.datetime.datetime.now() %}
      {% set dtstring = dt.strftime("%H%M%S%f") %}
      {% set suffix = suffix ~ dtstring %}
    {% endif %}
    {% set suffix_length = suffix|length %}
    {% set relation_max_name_length = base_relation.relation_max_name_length() %}
    {% if suffix_length > relation_max_name_length %}
        {% do exceptions.raise_compiler_error('Relation suffix is too long (' ~ suffix_length ~ ' characters). Maximum length is ' ~ relation_max_name_length ~ ' characters.') %}
    {% endif %}
    {% set identifier = base_relation.identifier[:relation_max_name_length - suffix_length] ~ suffix %}

    {{ return(base_relation.incorporate(path={"identifier": identifier })) }}

  {% endmacro %}

{% macro postgres__make_intermediate_relation(base_relation, suffix) %}
    {{ return(postgres__make_relation_with_suffix(base_relation, suffix, dstring=False)) }}
{% endmacro %}

{% macro postgres__make_temp_relation(base_relation, suffix) %}
    {% set temp_relation = postgres__make_relation_with_suffix(base_relation, suffix, dstring=True) %}
    {{ return(temp_relation.incorporate(path={"schema": none,
                                              "database": none})) }}
{% endmacro %}

{% macro postgres__make_backup_relation(base_relation, backup_relation_type, suffix) %}
    {% set backup_relation = postgres__make_relation_with_suffix(base_relation, suffix, dstring=False) %}
    {{ return(backup_relation.incorporate(type=backup_relation_type)) }}
{% endmacro %}

{#
  By using dollar-quoting like this, users can embed anything they want into their comments
  (including nested dollar-quoting), as long as they do not use this exact dollar-quoting
  label. It would be nice to just pick a new one but eventually you do have to give up.
#}
{% macro postgres_escape_comment(comment) -%}
  {% if comment is not string %}
    {% do exceptions.raise_compiler_error('cannot escape a non-string: ' ~ comment) %}
  {% endif %}
  {%- set magic = '$dbt_comment_literal_block$' -%}
  {%- if magic in comment -%}
    {%- do exceptions.raise_compiler_error('The string ' ~ magic ~ ' is not allowed in comments.') -%}
  {%- endif -%}
  {{ magic }}{{ comment }}{{ magic }}
{%- endmacro %}


{% macro postgres__alter_relation_comment(relation, comment) %}
  {% set escaped_comment = postgres_escape_comment(comment) %}
  comment on {{ relation.type }} {{ relation }} is {{ escaped_comment }};
{% endmacro %}


{% macro postgres__alter_column_comment(relation, column_dict) %}
  {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
  {% for column_name in column_dict if (column_name in existing_columns) %}
    {% set comment = column_dict[column_name]['description'] %}
    {% set escaped_comment = postgres_escape_comment(comment) %}
    comment on column {{ relation }}.{{ adapter.quote(column_name) if column_dict[column_name]['quote'] else column_name }} is {{ escaped_comment }};
  {% endfor %}
{% endmacro %}

{%- macro postgres__get_show_grant_sql(relation) -%}
  select grantee, privilege_type
  from {{ relation.information_schema('role_table_grants') }}
      where grantor = current_role
        and grantee != current_role
        and table_schema = '{{ relation.schema }}'
        and table_name = '{{ relation.identifier }}'
{%- endmacro -%}

{% macro postgres__copy_grants() %}
    {{ return(False) }}
{% endmacro %}


{% macro postgres__get_show_indexes_sql(relation) %}
    select
        i.relname                                   as name,
        m.amname                                    as method,
        ix.indisunique                              as "unique",
        array_to_string(array_agg(a.attname), ',')  as column_names
    from pg_index ix
    join pg_class i
        on i.oid = ix.indexrelid
    join pg_am m
        on m.oid=i.relam
    join pg_class t
        on t.oid = ix.indrelid
    join pg_namespace n
        on n.oid = t.relnamespace
    join pg_attribute a
        on a.attrelid = t.oid
        and a.attnum = ANY(ix.indkey)
    where t.relname = '{{ relation.identifier }}'
      and n.nspname = '{{ relation.schema }}'
      and t.relkind in ('r', 'm')
    group by 1, 2, 3
    order by 1, 2, 3
{% endmacro %}


{%- macro postgres__get_drop_index_sql(relation, index_name) -%}
    drop index if exists "{{ relation.schema }}"."{{ index_name }}"
{%- endmacro -%}
