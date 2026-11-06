{% macro run_pre_hooks() %}
  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}
{% endmacro %}

{% macro run_post_hooks() %}
  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {{ run_hooks(post_hooks, inside_transaction=False) }}
{% endmacro %}

{#- Databricks never opens a transaction, so the global run_hooks COMMIT would fail. -#}
-- funcsign: (list[hook], optional[bool]) -> string
{% macro run_hooks(hooks, inside_transaction=True) %}
  {%- for hook in hooks | selectattr('transaction', 'equalto', inside_transaction) -%}
    {%- set rendered = render(hook.get('sql')) | trim -%}
    {%- if (rendered | length) > 0 -%}
      {%- call statement(auto_begin=inside_transaction) -%}
        {{ rendered }}
      {%- endcall -%}
    {%- endif -%}
  {%- endfor -%}
{% endmacro %}
