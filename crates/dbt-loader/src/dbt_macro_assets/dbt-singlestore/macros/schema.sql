{% macro singlestore__generate_schema_name(custom_schema_name, node) -%}
    {# In SingleStore, databases and schemas are synonymous. All objects reside within target.schema. #}
    {%- set default_schema = target.schema -%}
    {%- if custom_schema_name is not none and custom_schema_name | trim != default_schema and node is not none -%}
        {{ log("SingleStore: custom schema '" ~ custom_schema_name ~ "' for model '" ~ node.name ~ "' mapped to table prefix within database '" ~ default_schema ~ "'.", info=False) }}
    {%- endif -%}
    {{ default_schema }}
{%- endmacro %}

{% macro singlestore__generate_alias_name(custom_alias_name=none, node=none) -%}
    {%- if custom_alias_name -%}
        {%- set base_alias = custom_alias_name | trim -%}
    {%- elif node.version -%}
        {%- set base_alias = node.name ~ "_v" ~ (node.version | replace(".", "_")) -%}
    {%- else -%}
        {%- set base_alias = node.name -%}
    {%- endif -%}

    {%- set custom_schema = none -%}
    {%- if node is not none -%}
        {%- if node.unrendered_config is defined and node.unrendered_config.get('schema') -%}
            {%- set custom_schema = node.unrendered_config.get('schema') | trim -%}
        {%- elif node.config is defined and node.config.get('schema') and node.config.get('schema') != target.schema -%}
            {%- set custom_schema = node.config.get('schema') | trim -%}
        {%- endif -%}
    {%- endif -%}

    {%- if custom_schema and custom_schema != target.schema -%}
        {{ custom_schema }}__{{ base_alias }}
    {%- else -%}
        {{ base_alias }}
    {%- endif -%}
{%- endmacro %}

{% macro singlestore__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create database if not exists {{ relation.without_identifier().render() }}
  {%- endcall -%}
{% endmacro %}

{% macro singlestore__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop database if exists {{ relation.without_identifier().render() }}
  {%- endcall -%}
{% endmacro %}

