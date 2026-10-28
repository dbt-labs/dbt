{%- macro check_model_is_table(model) -%}
    {%- if model.config.materialized != 'table' -%}
        {%- do exceptions.raise_compiler_error(
            "Model must use the table materialization. Please check any model overrides."
        ) -%}
    {%- endif -%}
{%- endmacro -%}