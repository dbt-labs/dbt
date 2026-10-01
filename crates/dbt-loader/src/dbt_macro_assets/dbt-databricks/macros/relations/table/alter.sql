{% macro apply_config_changeset(target_relation, model, configuration_changes, existing_relation=none) %}
    {{ log("Applying configuration changes to relation " ~ target_relation) }}
    {% if configuration_changes %}
      {% set comment = configuration_changes.changes.get("comment") %}
      {% set column_comments = configuration_changes.changes.get("column_comments") %}
      {% set column_tags = configuration_changes.changes.get("column_tags") %}
      {% set tags = configuration_changes.changes.get("tags") %}
      {% set tblproperties = configuration_changes.changes.get("tblproperties") %}
      {% set liquid_clustering = configuration_changes.changes.get("liquid_clustering")%}
      {% set constraints = configuration_changes.changes.get("constraints") %}
      {% set column_masks = configuration_changes.changes.get("column_masks") %}
      {% set row_filter = configuration_changes.changes.get("row_filter") %}
      {% if tags is not none %}
        {% do apply_tags(target_relation, tags.set_tags) %}
      {%- endif -%}
      {% if tblproperties is not none %}
        {% do apply_tblproperties(target_relation, tblproperties.tblproperties) %}
      {%- endif -%}
      {% if liquid_clustering is not none %}
        {% do apply_liquid_clustered_cols(target_relation, liquid_clustering, existing_relation) %}
      {%- endif -%}
      {% if comment %}
        {{ run_query_as(alter_relation_comment_sql(target_relation, comment.comment), 'alter_relation_comment', fetch_result=False) }}
      {% endif %}
      {% if column_comments %}
        {{ alter_column_comments(target_relation, column_comments.comments) }}
      {% endif %}
      {% if column_tags %}
        {{ apply_column_tags(target_relation, column_tags) }}
      {% endif %}
      {#-- Reconcile constraints only when the contract is enforced. --#}
      {% set contract_config = config.get('contract') %}
      {% if constraints and contract_config and contract_config.enforced %}
        {{ apply_constraints(target_relation, constraints) }}
      {% endif %}
      {% if column_masks %}
        {{ apply_column_masks(target_relation, column_masks) }}
      {% endif %}
      {% if row_filter %}
        {{ apply_row_filter(target_relation, row_filter) }}
      {% endif %}
    {%- endif -%}
{% endmacro %}
