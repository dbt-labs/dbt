{%- macro get_configuration_changes(existing_relation) -%}
    {%- set model_config = adapter.get_config_from_model(config.model) -%}
    {%- set existing_config = adapter.get_relation_config(existing_relation, model_config) -%}
    {%- set configuration_changes = model_config.get_changeset(existing_config) -%}
    {% do return(configuration_changes) %}
{%- endmacro -%}
