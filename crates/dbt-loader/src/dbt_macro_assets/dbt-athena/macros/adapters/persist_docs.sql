{% macro athena__persist_docs(relation, model, for_relation=true, for_columns=true) -%}
  {% set persist_relation_docs = for_relation and config.persist_relation_docs()%}
  {% set persist_column_docs = for_columns and config.persist_column_docs() and model.columns %}
  {% if persist_column_docs %}
    {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
    {% do validate_doc_columns(relation, model.columns, existing_columns) %}
  {% endif %}
  {% if persist_relation_docs or persist_column_docs %}
    {% do adapter.persist_docs_to_glue(
            relation=relation,
            model=model,
            persist_relation_docs=persist_relation_docs,
            persist_column_docs=persist_column_docs,
            skip_archive_table_version=true
         )
    %}
  {% endif %}
{% endmacro %}
