{% macro singlestore__generate_schema_name(custom_schema_name, node) -%}
    {# In SingleStore, databases are physical containers, while schemas act as table prefixes
       for developer and logical namespace isolation. #}
    {%- set target_schema = target.schema | default('', true) | trim -%}
    {%- set target_db = target.database | default('', true) | trim -%}
    {# If target.schema matches target.database, treat as no prefix (legacy profile compatibility) #}
    {%- if target_schema == target_db -%}
        {%- set default_prefix = "" -%}
    {%- else -%}
        {%- set default_prefix = target_schema -%}
    {%- endif -%}

    {%- if custom_schema_name is none or custom_schema_name | trim | length == 0 -%}
        {{ default_prefix }}
    {%- elif default_prefix | length > 0 -%}
        {{ default_prefix }}_{{ custom_schema_name | trim }}
    {%- else -%}
        {{ custom_schema_name | trim }}
    {%- endif -%}
{%- endmacro %}

{% macro singlestore__generate_alias_name(custom_alias_name=none, node=none) -%}
    {%- if custom_alias_name -%}
        {%- set base_alias = custom_alias_name | trim -%}
    {%- elif node is not none and node.version -%}
        {%- set base_alias = node.name ~ "_v" ~ (node.version | replace(".", "_")) -%}
    {%- elif node is not none -%}
        {%- set base_alias = node.name -%}
    {%- else -%}
        {%- set base_alias = "" -%}
    {%- endif -%}

    {%- set schema_prefix = "" -%}
    {%- if node is not none and node.schema is defined and node.schema -%}
        {%- set raw_schema = node.schema | trim -%}
        {%- set node_db = node.database | default('', true) | trim -%}
        {# Do not prefix if schema matches database name (legacy guardrail) #}
        {%- if raw_schema | length > 0 and raw_schema != node_db -%}
            {%- set schema_prefix = raw_schema -%}
        {%- endif -%}
    {%- endif -%}

    {%- if schema_prefix | length > 0 -%}
        {{ schema_prefix }}_{{ base_alias }}
    {%- else -%}
        {{ base_alias }}
    {%- endif -%}
{%- endmacro %}

{% macro singlestore__create_schema(relation) -%}
  {# In SingleStore, database and schema are synonymous and the target database is already provisioned.
     Matches dbt-singlestore 1.x safe no-op. #}
  {%- call statement('create_schema') -%}
    SELECT 'create_schema'
  {%- endcall -%}
{% endmacro %}

{% macro singlestore__drop_schema(relation) -%}
  {# In SingleStore, schema does not have a physical representation separate from database.
     Matches dbt-singlestore 1.x cache drop behavior. #}
  {%- call statement('drop_schema') -%}
    SELECT 'drop_schema'
  {%- endcall -%}
{% endmacro %}
