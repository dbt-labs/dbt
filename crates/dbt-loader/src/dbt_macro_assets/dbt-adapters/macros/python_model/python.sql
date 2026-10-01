-- funcsign: (string) -> string
{% macro resolve_model_name(input_model_name) %}
    {{ return(adapter.dispatch('resolve_model_name', 'dbt')(input_model_name)) }}
{% endmacro %}

-- funcsign: (string) -> string
{%- macro default__resolve_model_name(input_model_name) -%}
    {{  input_model_name | string | replace('"', '\"') }}
{%- endmacro -%}

-- funcsign: (model) -> string
{% macro build_ref_function(model) %}

    {%- set ref_dict = {} -%}
    {%- for _ref in model.refs -%}
        {% set _ref_args = [_ref.get('package'), _ref['name']] if _ref.get('package') else [_ref['name'],] %}
        {%- set resolved = ref(*_ref_args, v=_ref.get('version')) -%}

        {#
            We want to get the string of the returned relation by calling .render() in order to skip sample/empty
            mode rendering logic. However, people override the default ref macro, and often return a string instead
            of a relation (like the ref macro does by default). Thus, to make sure we dont blow things up, we have
            to ensure the resolved relation has a .render() method.
        #}
        {%- if resolved.render is defined and resolved.render is callable -%}
            {%- set resolved = resolved.render() -%}
        {%- endif -%}

        {%- if _ref.get('version') -%}
            {% do _ref_args.extend(["v" ~ _ref['version']]) %}
        {%- endif -%}
       {%- do ref_dict.update({_ref_args | join('.'): resolve_model_name(resolved)}) -%}
    {%- endfor -%}

def ref(*args, **kwargs):
    refs = {{ ref_dict | tojson }}
    key = '.'.join(args)
    version = kwargs.get("v") or kwargs.get("version")
    if version:
        key += f".v{version}"
    dbt_load_df_function = kwargs.get("dbt_load_df_function")
    return dbt_load_df_function(refs[key])

{% endmacro %}

{% macro build_source_function(model) %}

    {%- set source_dict = {} -%}
    {%- for _source in model.sources -%}
        {%- set resolved = source(*_source) -%}
        {%- do source_dict.update({_source | join('.'): resolve_model_name(resolved)}) -%}
    {%- endfor -%}

def source(*args, dbt_load_df_function):
    sources = {{ source_dict | tojson }}
    key = '.'.join(args)
    return dbt_load_df_function(sources[key])

{% endmacro %}

{% macro build_config_dict(model) %}
    {%- set config_dict = {} -%}
    {% set config_dbt_used = zip(model.config.config_keys_used, model.config.config_keys_defaults) | list %}
    {%- for key, default in config_dbt_used -%}
        {# weird type testing with enum, would be much easier to write this logic in Python! #}
        {%- if key == "language" -%}
          {%- set value = "python" -%}
        {%- endif -%}
        {%- set value = model.config.get(key, default) -%}
        {%- do config_dict.update({key: value}) -%}
    {%- endfor -%}
    {# Handle dbt.config.meta_get() calls - use separate dict to avoid overwriting native configs #}
    {%- set meta_dict = {} -%}
    {%- if model.config.meta_keys_used -%}
        {% set meta_dbt_used = zip(model.config.meta_keys_used, model.config.meta_keys_defaults) | list %}
        {%- for key, default in meta_dbt_used -%}
            {%- if model.config.meta and key in model.config.meta -%}
                {%- set value = model.config.meta[key] -%}
            {%- else -%}
                {%- set value = default -%}
            {%- endif -%}
            {%- do meta_dict.update({key: value}) -%}
        {%- endfor -%}
    {%- endif -%}
config_dict = {{ config_dict }}
meta_dict = {{ meta_dict }}
{% endmacro %}

{% macro py_script_postfix(model) %}
# This part is user provided model code
# you will need to copy the next section to run the code
# COMMAND ----------
# this part is dbt logic for get ref work, do not modify

{{ build_ref_function(model ) }}
{{ build_source_function(model ) }}
{{ build_config_dict(model) }}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

    @staticmethod
    def meta_get(key, default=None):
        return meta_dict.get(key, default)

class this:
    """dbt.this() or dbt.this.identifier"""
    database = "{{ this.database }}"
    schema = "{{ this.schema }}"
    identifier = "{{ this.identifier }}"
    {% set this_relation_name = resolve_model_name(this) %}
    def __repr__(self):
        return '{{ this_relation_name  }}'


class dbtObj:
    def __init__(self, load_df_function) -> None:
        self.source = lambda *args: source(*args, dbt_load_df_function=load_df_function)
        self.ref = lambda *args, **kwargs: ref(*args, **kwargs, dbt_load_df_function=load_df_function)
        self.config = config
        self.this = this()
        self.is_incremental = {{ is_incremental() }}

# COMMAND ----------
{{py_script_comment()}}
{% endmacro %}

{#-- entry point for add instuctions for running compiled_code --#}
{%macro py_script_comment()%}
{%endmacro%}
