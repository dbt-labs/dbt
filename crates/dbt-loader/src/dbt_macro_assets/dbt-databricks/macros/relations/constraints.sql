{# Persist table-level and column-level constraints. #}
{% macro persist_constraints(relation, model) %}
  {{ return(adapter.dispatch('persist_constraints', 'dbt')(relation, model)) }}
{% endmacro %}

{% macro databricks__persist_constraints(relation, model) %}
  {%- set contract_config = config.get('contract') -%}
  {% set has_model_contract = contract_config and contract_config.enforced %}
  {% set has_databricks_constraints = config.get('persist_constraints', False) %}

  {% if (has_model_contract or has_databricks_constraints) %}
    {% if adapter.resolve_file_format(config) != 'delta' %}
      {# Constraints are only supported for delta tables #}
      {{ exceptions.warn("Constraints not supported for file format: " ~ adapter.resolve_file_format(config)) }}
    {% elif relation.is_view %}
      {# Constraints are not supported for views. This point in the code should not have been reached. #}
      {{ exceptions.raise_compiler_error("Constraints not supported for views.") }}
    {% elif is_incremental() %}
      {# Constraints are not applied for incremental updates. This point in the code should not have been reached #}
      {{ exceptions.raise_compiler_error("Constraints are not applied for incremental updates. Full refresh is required to update constraints.") }}
    {% else %}
      {#- DIVERGENCE BEGIN: drop any PK inherited from a SHALLOW CLONE before applying
          model constraints. SHALLOW CLONE preserves and renames the source's primary key,
          so a subsequent ALTER TABLE ADD CONSTRAINT fails with "table already has a PRIMARY
          KEY constraint". Upstream dbt-databricks is not affected because it does not use
          SHALLOW CLONE. Only applicable to Unity Catalog (not Hive Metastore).
          Gated on dbt_version: under dbt-core (1.x) SHALLOW CLONE is not used, so no-op. -#}
      {% if dbt_version.startswith('2.') and not relation.is_hive_metastore() %}
        {% set existing_pks = fetch_primary_key_constraints(relation) %}
        {% if existing_pks.rows | length > 0 %}
          {% for row in existing_pks %}
            {% if loop.first %}
              {% call statement('drop_inherited_pk') %}
                ALTER TABLE {{ relation.render() }} DROP CONSTRAINT {{ row['constraint_name'] }} CASCADE;
              {% endcall %}
            {% endif %}
          {% endfor %}
        {% endif %}
      {% endif %}
      {#- DIVERGENCE END -#}
      {% do alter_column_set_constraints(relation, model) %}
      {% do alter_table_add_constraints(relation, model) %}
    {% endif %}
  {% endif %}
{% endmacro %}

{% macro apply_alter_constraints(relation) %}
  {%- for constraint in relation.alter_constraints -%}
    {% call statement('add constraint') %}
      ALTER TABLE {{ relation.render() }} ADD {{ constraint.render() }}
    {% endcall %}
  {%- endfor -%}
{% endmacro %}

{% macro alter_table_add_constraints(relation, constraints) %}
  {{ return(adapter.dispatch('alter_table_add_constraints', 'dbt')(relation, constraints)) }}
{% endmacro %}

{% macro databricks__alter_table_add_constraints(relation, model) %}
    {% set constraints = get_model_constraints(model) %}
    {#- DIVERGENCE BEGIN: skip model-level PK constraints that already exist, for the
        same SHALLOW CLONE reason as in databricks__persist_constraints above.
        Gated on dbt_version: under dbt-core (1.x) SHALLOW CLONE is not used, so no-op. -#}
    {% if dbt_version.startswith('2.') %}
      {% set wants_pk = constraints | selectattr('type', 'eq', 'primary_key') | list | length > 0 %}
      {% if wants_pk and not relation.is_hive_metastore() %}
        {% set existing_pks = fetch_primary_key_constraints(relation) %}
        {% if existing_pks.rows | length > 0 %}
          {% set constraints = constraints | rejectattr('type', 'eq', 'primary_key') | list %}
        {% endif %}
      {% endif %}
    {% endif %}
    {#- DIVERGENCE END -#}
    {% set statements = get_constraints_sql(relation, constraints, model) %}
    {% for stmt in statements %}
      {% call statement() %}
        {{ stmt }}
      {% endcall %}
    {% endfor %}
{% endmacro %}

{% macro get_model_constraints(model) %}
  {% set constraints = model.get('constraints', []) %}
  {% if config.get('persist_constraints', False) and model.get('meta', {}).get('constraints') is sequence %}
    {# Databricks constraints implementation.  Constraints are in the meta property. #}
    {% set db_constraints = model.get('meta', {}).get('constraints', []) %}
    {% set constraints = databricks_constraints_to_dbt(db_constraints) %}
  {% endif %}
  {{ return(constraints) }}
{% endmacro %}

{% macro get_column_constraints(column) %}
  {% set constraints = column.get('constraints', []) %}
  {% if config.get('persist_constraints', False) and column.get('meta', {}).get('constraint') %}
    {# Databricks constraints implementation.  Constraint is in the meta property. #}
    {% set db_constraints = [column.get('meta', {}).get('constraint')] %}
    {% set constraints = databricks_constraints_to_dbt(db_constraints, column) %}
  {% endif %}
  {{ return(constraints) }}
{% endmacro %}

{% macro alter_column_set_constraints(relation, column_dict) %}
  {{ return(adapter.dispatch('alter_column_set_constraints', 'dbt')(relation, column_dict)) }}
{% endmacro %}

{% macro databricks__alter_column_set_constraints(relation, model) %}
  {% set column_dict = model.columns %}
  {% for column_name in column_dict %}
    {% set column = column_dict[column_name] %}
    {% set constraints = get_column_constraints(column)  %}
    {% set statements = get_constraints_sql(relation, constraints, model, column) %}
    {% for stmt in statements %}
      {% call statement() %}
        {{ stmt }}
      {% endcall %}
    {% endfor %}
  {% endfor %}
{% endmacro %}

{% macro get_constraints_sql(relation, constraints, model, column={}) %}
  {% set statements = [] %}
  -- Hack so that not null constraints will be applied before other constraints
  {% for constraint in constraints|selectattr('type', 'eq', 'not_null') %}
    {% if constraint %}
      {% set constraint_statements = get_constraint_sql(relation, constraint, model, column) %}
      {% for statement in constraint_statements %}
        {% if statement %}
          {% do statements.append(statement) %}
        {% endif %}
      {% endfor %}
    {% endif %}
  {% endfor %}
  {% for constraint in constraints|rejectattr('type', 'eq', 'not_null') %}
    {% if constraint %}
      {% set constraint_statements = get_constraint_sql(relation, constraint, model, column) %}
      {% for statement in constraint_statements %}
        {% if statement %}
          {% do statements.append(statement) %}
        {% endif %}
      {% endfor %}
    {% endif %}
  {% endfor %}

  {{ return(statements) }}
{% endmacro %}

{% macro get_constraint_sql(relation, constraint, model, column={}) %}
  {% set statements = [] %}
  {% set type = constraint.get('type', '') %}

  {% if type == 'check' %}
    {% set expression = constraint.get('expression', '') %}
    {% if not expression %}
      {{ exceptions.raise_compiler_error('Invalid check constraint expression') }}
    {% endif %}

    {% set name = constraint.get('name') %}
    {% if not name %}
      {% if local_md5 %}
        {{ exceptions.warn("Constraint of type " ~ type ~ " with no `name` provided. Generating hash instead for relation " ~ relation.identifier) }}
        {%- set name = local_md5 (relation.identifier ~ ";" ~ column.get('name', '') ~ ";" ~ expression ~ ";") -%}
      {% else %}
        {{ exceptions.raise_compiler_error("Constraint of type " ~ type ~ " with no `name` provided, and no md5 utility.") }}
      {% endif %}
    {% endif %}
    {% set stmt = "alter table " ~ relation.render() ~ " add constraint " ~ name ~ " check (" ~ expression ~ ");" %}
    {% do statements.append(stmt) %}
  {% elif type == 'not_null' %}
    {% set column_names = constraint.get('columns', []) %}
    {% if column and not column_names %}
      {% set column_names = [column['name']] %}
    {% endif %}
    {% for column_name in column_names %}
      {% set column = model.get('columns', {}).get(column_name) %}
      {% if column %}
        {% set quoted_name = adapter.quote(column['name']) %}
        {% set stmt = "alter table " ~ relation.render() ~ " change column " ~ quoted_name ~ " set not null " ~ (constraint.expression or "") ~ ";" %}
        {% do statements.append(stmt) %}
      {% else %}
        {{ exceptions.warn('not_null constraint on invalid column: ' ~ column_name) }}
      {% endif %}
    {% endfor %}
  {% elif type == 'primary_key' %}
    {% if constraint.get('warn_unenforced') %}
      {{ exceptions.warn("unenforced constraint type: " ~ type)}}
    {% endif %}
    {% set column_names = constraint.get('columns', []) %}
    {% if column and not column_names %}
      {% set column_names = [column['name']] %}
    {% endif %}
    {% set quoted_names = [] %}
    {% for column_name in column_names %}
      {% set column = model.get('columns', {}).get(column_name) %}
      {% if not column %}
        {{ exceptions.warn('Invalid primary key column: ' ~ column_name) }}
      {% else %}
        {% set quoted_name = adapter.quote(column['name']) %}
        {% do quoted_names.append(quoted_name) %}
      {% endif %}
    {% endfor %}

    {% set joined_names = quoted_names|join(", ") %}
    {% set pk_expression = constraint.get('expression') %}

    {% set name = constraint.get('name') %}
    {% if not name %}
      {% if local_md5 %}
        {{ exceptions.warn("Constraint of type " ~ type ~ " with no `name` provided. Generating hash instead for relation " ~ relation.identifier) }}
        {%- set hash_input = "primary_key;" ~ relation.identifier ~ ";" ~ column_names ~ ";" -%}
        {%- if pk_expression -%}
          {%- set hash_input = hash_input ~ pk_expression ~ ";" -%}
        {%- endif -%}
        {%- set name = local_md5(hash_input) -%}
      {% else %}
        {{ exceptions.raise_compiler_error("Constraint of type " ~ type ~ " with no `name` provided, and no md5 utility.") }}
      {% endif %}
    {% endif %}
    {% set pk_suffix = (" " ~ pk_expression) if pk_expression else "" %}
    {% set stmt = "alter table " ~ relation.render() ~ " add constraint " ~ name ~ " primary key(" ~ joined_names ~ ")" ~ pk_suffix ~ ";" %}
    {% do statements.append(stmt) %}
  {% elif type == 'foreign_key' %}

    {% if constraint.get('warn_unenforced') %}
      {{ exceptions.warn("unenforced constraint type: " ~ constraint.type)}}
    {% endif %}

    {% set name = constraint.get('name') %}
    
    {% if constraint.get('expression') %}

      {% if not name %}
        {% if local_md5 %}
          {{ exceptions.warn("Constraint of type " ~ type ~ " with no `name` provided. Generating hash instead for relation " ~ relation.identifier) }}
          {%- set name = local_md5("foreign_key;" ~ relation.identifier ~ ";" ~ constraint.get('expression') ~ ";") -%}
        {% else %}
          {{ exceptions.raise_compiler_error("Constraint of type " ~ type ~ " with no `name` provided, and no md5 utility.") }}
        {% endif %}    
      {% endif %}

      {% set stmt = "alter table " ~ relation.render() ~ " add constraint " ~ name ~ " foreign key" ~ constraint.get('expression') %}
    {% else %}
      {% set column_names = constraint.get('columns', []) %}
      {% if column and not column_names %}
        {% set column_names = [column['name']] %}
      {% endif %}
      {% set quoted_names = [] %}
      {% for column_name in column_names %}
        {% set column = model.get('columns', {}).get(column_name) %}
        {% if not column %}
          {{ exceptions.warn('Invalid foreign key column: ' ~ column_name) }}
        {% else %}
          {% set quoted_name = adapter.quote(column['name']) %}
          {% do quoted_names.append(quoted_name) %}
        {% endif %}
      {% endfor %}

      {% set joined_names = quoted_names|join(", ") %}

      {% set parent = constraint.get('to') %}
      {% if not parent %}
        {{ exceptions.raise_compiler_error('No parent table defined for foreign key: ' ~ expression) }}
      {% endif %}
      {% if not "." in parent %}
        {% set parent_relation = api.Relation.create(database=relation.database, schema=relation.schema, identifier=parent, type='table') %}
        {% set parent = parent_relation.render() %}
      {% endif %}

      {% if not name %}
        {% if local_md5 %}
          {{ exceptions.warn("Constraint of type " ~ type ~ " with no `name` provided. Generating hash instead for relation " ~ relation.identifier) }}
          {%- set name = local_md5("foreign_key;" ~ relation.identifier ~ ";" ~ column_names ~ ";" ~ parent ~ ";") -%}
        {% else %}
          {{ exceptions.raise_compiler_error("Constraint of type " ~ type ~ " with no `name` provided, and no md5 utility.") }}
        {% endif %}    
      {% endif %}

      {% set stmt = "alter table " ~ relation.render() ~ " add constraint " ~ name ~ " foreign key(" ~ joined_names ~ ") references " ~ parent %}
      {% set parent_columns = constraint.get('to_columns') %}
      {% if parent_columns %}
        {% set quoted_parent_columns = [] %}
        {% for parent_column in parent_columns %}
          {% do quoted_parent_columns.append(adapter.quote(parent_column)) %}
        {% endfor %}
        {% set stmt = stmt ~ "(" ~ quoted_parent_columns|join(", ") ~ ")"%}
      {% endif %}
    {% endif %}
    {% set stmt = stmt ~ ";" %}
    {% do statements.append(stmt) %}
  {% elif type == 'custom' %}
    {% set expression = constraint.get('expression', '') %}
    {% if not expression %}
      {{ exceptions.raise_compiler_error('Missing custom constraint expression') }}
    {% endif %}

    {% set name = constraint.get('name') %}
    {% set expression = constraint.get('expression') %}
    {% if not name %}
      {% if local_md5 %}
        {{ exceptions.warn("Constraint of type " ~ type ~ " with no `name` provided. Generating hash instead for relation " ~ relation.identifier) }}
        {%- set name = local_md5 (relation.identifier ~ ";" ~ expression ~ ";") -%}
      {% else %}
        {{ exceptions.raise_compiler_error("Constraint of type " ~ type ~ " with no `name` provided, and no md5 utility.") }}
      {% endif %}
    {% endif %}
    {% set stmt = "alter table " ~ relation.render() ~ " add constraint " ~ name ~ " " ~ expression ~ ";" %}
    {% do statements.append(stmt) %}
  {% elif constraint.get('warn_unsupported') %}
    {{ exceptions.warn("unsupported constraint type: " ~ constraint.type)}}
  {% endif %}

  {{ return(statements) }}
{% endmacro %}

{% macro databricks_constraints_to_dbt(constraints, column) %}
  {# convert constraints defined using the original databricks format #}
  {% set dbt_constraints = [] %}
  {% for constraint in constraints %}
    {% if constraint.get and constraint.get('type') %}
      {# already in model contract format #}
      {% do dbt_constraints.append(constraint) %}
    {% else %}
      {% if column %}
        {% if constraint == "not_null" %}
          {% do dbt_constraints.append({"type": "not_null", "columns": [column.get('name')]}) %}
        {% else %}
          {{ exceptions.raise_compiler_error('Invalid constraint for column ' ~ column.get('name', "") ~ '. Only `not_null` is supported.') }}
        {% endif %}
      {% else %}
        {% set name = constraint['name'] %}
        {% if not name %}
          {{ exceptions.raise_compiler_error('Invalid check constraint name') }}
        {% endif %}
        {% set condition = constraint['condition'] %}
        {% if not condition %}
          {{ exceptions.raise_compiler_error('Invalid check constraint condition') }}
        {% endif %}
        {% do dbt_constraints.append({"name": name, "type": "check", "expression": condition}) %}
      {% endif %}
    {% endif %}
  {% endfor %}

  {{ return(dbt_constraints) }}
{% endmacro %}
