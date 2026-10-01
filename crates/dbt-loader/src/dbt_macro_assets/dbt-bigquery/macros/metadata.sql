{% macro bigquery__get_relation_last_modified(information_schema, relations) -%}

  {% set unique_schemas = (relations | map(attribute='schema')) | list | unique %}

  {% set location = adapter.get_dataset_location(relations[0]) %}
  {% set information_schema = relations[0].incorporate(location=location).information_schema("TABLE_STORAGE") %}

  {%- call statement('last_modified', fetch_result=True) -%}
        select table_schema as schema,
        table_name as identifier,
        storage_last_modified_time as last_modified,
               {{ current_timestamp() }} as snapshotted_at
        from {{ information_schema }}
        where (upper(table_schema) IN (
            {%- for unique_schema in unique_schemas -%}
            upper('{{ unique_schema }}'){%- if not loop.last %},{% endif -%}
            {%- endfor -%}
            )
        )
        and (
          {%- for relation in relations -%}
            (upper(table_schema) = upper('{{ relation.schema }}') and
             upper(table_name) = upper('{{ relation.identifier }}')){%- if not loop.last %} or {% endif -%}
          {%- endfor -%}
        )
  {%- endcall -%}

  {{ return(load_result('last_modified')) }}

{% endmacro %}
