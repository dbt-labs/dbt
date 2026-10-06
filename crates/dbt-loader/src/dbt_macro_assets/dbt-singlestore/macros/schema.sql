{% macro singlestore__generate_schema_name(custom_schema_name, node) -%}
    {# In SingleStore, databases and schemas are synonymous. All objects reside within target database unless database is configured. #}
    {%- set default_schema = target.schema | default(target.database, true) | trim -%}
    {%- if not default_schema -%}
        {%- set default_schema = target.database | trim -%}
    {%- endif -%}
    {%- if node is not none -%}
        {%- if node.unrendered_config is defined and node.unrendered_config.get('database') -%}
            {%- set default_schema = node.unrendered_config.get('database') | trim -%}
        {%- elif node.config is defined and node.config.get('database') -%}
            {%- set default_schema = node.config.get('database') | trim -%}
        {%- endif -%}
    {%- endif -%}
    {%- if custom_schema_name is not none and custom_schema_name | trim | length > 0 and custom_schema_name | trim != default_schema and node is not none -%}
        {{ log("SingleStore: custom schema '" ~ custom_schema_name ~ "' for model '" ~ node.name ~ "' mapped to table prefix within database '" ~ default_schema ~ "'.", info=False) }}
    {%- endif -%}
    {{ default_schema }}
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

    {%- set default_db = target.schema | default(target.database, true) | trim -%}
    {%- if not default_db -%}
        {%- set default_db = target.database | trim -%}
    {%- endif -%}
    {%- if node is not none -%}
        {%- if node.unrendered_config is defined and node.unrendered_config.get('database') -%}
            {%- set configured_db = node.unrendered_config.get('database') | trim -%}
        {%- elif node.config is defined and node.config.get('database') -%}
            {%- set configured_db = node.config.get('database') | trim -%}
        {%- else -%}
            {%- set configured_db = default_db -%}
        {%- endif -%}
    {%- else -%}
        {%- set configured_db = default_db -%}
    {%- endif -%}

    {%- set custom_schema = none -%}
    {%- if node is not none -%}
        {%- if node.unrendered_config is defined and node.unrendered_config.get('schema') -%}
            {%- set raw_schema = node.unrendered_config.get('schema') | trim -%}
            {%- if raw_schema | length > 0 and raw_schema != configured_db and raw_schema != default_db and raw_schema != target.database and raw_schema != target.schema -%}
                {%- set custom_schema = raw_schema -%}
            {%- endif -%}
        {%- elif node.config is defined and node.config.get('schema') -%}
            {%- set raw_schema = node.config.get('schema') | trim -%}
            {%- if raw_schema | length > 0 and raw_schema != configured_db and raw_schema != default_db and raw_schema != target.database and raw_schema != target.schema -%}
                {%- set custom_schema = raw_schema -%}
            {%- endif -%}
        {%- endif -%}
    {%- endif -%}

    {%- if custom_schema and custom_schema | length > 0 -%}
        {{ custom_schema }}_{{ base_alias }}
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
