{% macro sqlserver__make_temp_relation(base_relation, suffix='__dbt_temp') %}
    {%- set temp_identifier = base_relation.identifier ~ suffix -%}
    {%- set temp_relation = base_relation.incorporate(
                                path={"identifier": temp_identifier}) -%}

    {{ return(temp_relation) }}
{% endmacro %}

{% macro sqlserver__get_drop_sql(relation) -%}
  {% if relation.type == 'view' -%}
      {#- auto_begin=false, matching dbt-adapters' own relations/drop.sql: a
          read-only lookup must not open the ambient transaction. This is the
          first statement of the temp-relation build (create.sql ->
          adapter.drop_relation), so with the default auto_begin=True it held
          that build's sys.sysschobjs X locks until the materialization
          committed, deadlocking a concurrent worker (error 1205). -#}
      {% call statement('find_references', fetch_result=true, auto_begin=false) %}
        {{ get_use_database_sql(relation.database) }}
        select
            sch.name as schema_name,
            obj.name as view_name
        from sys.sql_expression_dependencies refs {{ information_schema_hints() }}
        inner join sys.objects obj {{ information_schema_hints() }}
        on refs.referencing_id = obj.object_id
        inner join sys.schemas sch {{ information_schema_hints() }}
        on obj.schema_id = sch.schema_id
        where refs.referenced_database_name = '{{ relation.database }}'
        and refs.referenced_schema_name = '{{ relation.schema }}'
        and refs.referenced_entity_name = '{{ relation.identifier }}'
        and obj.type = 'V'
        {{ get_query_options() }}
      {% endcall %}
      {% set references = load_result('find_references')['data'] %}
      {% for reference in references -%}
        -- dropping referenced view {{ reference[0] }}.{{ reference[1] }}
        {% do adapter.drop_relation
          (api.Relation.create(
            identifier = reference[1], schema = reference[0], database = relation.database, type='view'
          ))%}
      {% endfor %}
    {% elif relation.type == 'table'%}
      {% set object_id_type = 'U' %}
    {%- else -%}
        {{ exceptions.raise_not_implemented('Invalid relation being dropped: ' ~ relation) }}
    {% endif %}
    {{ get_use_database_sql(relation.database) }}
    EXEC('DROP {{ relation.type }} IF EXISTS {{ relation.include(database=False) }};');
{% endmacro %}

{% macro sqlserver__rename_relation(from_relation, to_relation) -%}
  {% call statement('rename_relation') -%}
     {{ get_use_database_sql(from_relation.database) }}
      {#- @objname takes a quoted, qualified name; @newname must stay bare -#}
      EXEC sp_rename '{{ escape_single_quotes(from_relation.include(database=False)) }}', '{{ escape_single_quotes(to_relation.identifier) }}'
  {%- endcall %}
{% endmacro %}

{% macro sqlserver__truncate_relation(relation) -%}
  {% call statement('truncate_relation') -%}
        truncate table {{ relation }}
    {%- endcall %}
{% endmacro %}
