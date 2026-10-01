{% macro fetch_non_null_constraint_columns(relation) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Incremental application of constraints is not supported for Hive Metastore") }}
  {%- endif %}
  {% call statement('list_non_null_constraint_columns', fetch_result=True) -%}
    {{ fetch_non_null_constraint_columns_sql(relation) }}
  {% endcall %}
  {% do return(load_result('list_non_null_constraint_columns').table) %}
{%- endmacro -%}

{% macro fetch_non_null_constraint_columns_sql(relation) -%}
  SELECT column_name
  FROM `{{ relation.database|lower }}`.`information_schema`.`columns`
  WHERE table_catalog = '{{ relation.database|lower }}' 
    AND table_schema = '{{ relation.schema|lower }}'
    AND table_name = '{{ relation.identifier|lower }}'
    AND is_nullable = 'NO';
{%- endmacro -%}

{% macro fetch_primary_key_constraints(relation) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Incremental application of constraints is not supported for Hive Metastore") }}
  {%- endif %}
  {% call statement('list_primary_key_constraints', fetch_result=True) -%}
    {{ fetch_primary_key_constraints_sql(relation) }}
  {% endcall %}
  {% do return(load_result('list_primary_key_constraints').table) %}
{%- endmacro -%}

{% macro fetch_primary_key_constraints_sql(relation) -%}
  SELECT kcu.constraint_name, kcu.column_name
  FROM `{{ relation.database|lower }}`.information_schema.key_column_usage kcu
  WHERE kcu.table_catalog = '{{ relation.database|lower }}' 
    AND kcu.table_schema = '{{ relation.schema|lower }}'
    AND kcu.table_name = '{{ relation.identifier|lower }}' 
    AND kcu.constraint_name = (
      SELECT constraint_name
      FROM `{{ relation.database|lower }}`.information_schema.table_constraints
      WHERE table_catalog = '{{ relation.database|lower }}'
        AND table_schema = '{{ relation.schema|lower }}'
        AND table_name = '{{ relation.identifier|lower }}' 
        AND constraint_type = 'PRIMARY KEY'
    )
  ORDER BY kcu.ordinal_position;
{%- endmacro -%}

{% macro fetch_foreign_key_constraints(relation) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Incremental application of constraints is not supported for Hive Metastore") }}
  {%- endif %}
  {% call statement('list_foreign_key_constraints', fetch_result=True) -%}
    {{ fetch_foreign_key_constraints_sql(relation) }}
  {% endcall %}
  {% do return(load_result('list_foreign_key_constraints').table) %}
{%- endmacro -%}

{% macro fetch_foreign_key_constraints_sql(relation) -%}
  SELECT
    kcu.constraint_name,
    kcu.column_name AS from_column,
    ukcu.table_catalog AS to_catalog,
    ukcu.table_schema AS to_schema,
    ukcu.table_name AS to_table,
    ukcu.column_name AS to_column
  FROM `{{ relation.database|lower }}`.information_schema.key_column_usage kcu
  JOIN `{{ relation.database|lower }}`.information_schema.referential_constraints rc
    ON kcu.constraint_name = rc.constraint_name
  JOIN `{{ relation.database|lower }}`.information_schema.key_column_usage ukcu
    ON rc.unique_constraint_name = ukcu.constraint_name
    AND kcu.ordinal_position = ukcu.ordinal_position
  WHERE kcu.table_catalog = '{{ relation.database|lower }}'
    AND kcu.table_schema = '{{ relation.schema|lower }}'
    AND kcu.table_name = '{{ relation.identifier|lower }}'
    AND kcu.constraint_name IN (
      SELECT constraint_name
      FROM `{{ relation.database|lower }}`.information_schema.table_constraints
      WHERE table_catalog = '{{ relation.database|lower }}'
        AND table_schema = '{{ relation.schema|lower }}'
        AND table_name = '{{ relation.identifier|lower }}'
        AND constraint_type = 'FOREIGN KEY'
    )
  ORDER BY kcu.ordinal_position;
{%- endmacro -%}

{% macro apply_constraints(relation, constraints) -%}
  {{ log("Applying constraints to relation " ~ constraints) }}
  {%- if constraints and relation.is_hive_metastore() -%}
    {{ exceptions.raise_compiler_error("Constraints are only supported for Unity Catalog") }}
  {%- endif -%}
  {# Order matters here because key constraints depend on non-null constraints #} 
  {%- if constraints.unset_constraints %}
    {%- for constraint in constraints.unset_constraints -%}
      {%- call statement('main') -%}
        {{ alter_unset_constraint(relation, constraint) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
  {%- if constraints.unset_non_nulls %}
    {%- for column in constraints.unset_non_nulls -%}
      {%- call statement('main') -%}
        {{ alter_unset_non_null_constraint(relation, column) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
  {%- if constraints.set_non_nulls %}
    {%- for column in constraints.set_non_nulls -%}
      {%- call statement('main') -%}
        {{ alter_set_non_null_constraint(relation, column) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
  {%- if constraints.set_constraints %}
    {%- for constraint in constraints.set_constraints -%}
      {%- call statement('main') -%}
        {{ alter_set_constraint(relation, constraint) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
{%- endmacro -%}

{% macro alter_set_non_null_constraint(relation, column) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }} ALTER COLUMN {{ adapter.quote(column) }} SET NOT NULL;
  {#- DIVERGENCE END -#}
{%- endmacro -%}

{% macro alter_unset_non_null_constraint(relation, column) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }} ALTER COLUMN {{ adapter.quote(column) }} DROP NOT NULL;
  {#- DIVERGENCE END -#}
{%- endmacro -%}

{% macro alter_set_constraint(relation, constraint) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }} ADD {{ constraint.render() }};
  {#- DIVERGENCE END -#}
{%- endmacro -%}

{% macro alter_unset_constraint(relation, constraint) -%}
  {% set constraint_type = constraint.type %}
  {% if constraint_type == 'primary_key' %}
    {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
    ALTER {{ render_type(relation.type) }} {{ relation.render() }} DROP CONSTRAINT {{ constraint.name }} CASCADE;
    {#- DIVERGENCE END -#}
  {% else %}
    {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
    ALTER {{ render_type(relation.type) }} {{ relation.render() }} DROP CONSTRAINT IF EXISTS {{ constraint.name }};
    {#- DIVERGENCE END -#}
  {% endif %}
{%- endmacro -%}
