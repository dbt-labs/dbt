{% macro run_pre_hooks() %}
  {{ run_hooks(pre_hooks, inside_transaction=False) }}
  {{ run_hooks(pre_hooks, inside_transaction=True) }}
{% endmacro %}

{% macro run_post_hooks() %}
  {{ run_hooks(post_hooks, inside_transaction=True) }}
  {{ run_hooks(post_hooks, inside_transaction=False) }}
{% endmacro %}

{#- Overrides the global run_hooks, whose outside-transaction pass sends a literal `commit;`
    that Databricks rejects because the adapter never opens a transaction. -#}
-- funcsign: (list[hook], optional[bool]) -> string
{% macro run_hooks(hooks, inside_transaction=True) %}
  {%- set selected = hooks | selectattr('transaction', 'equalto', inside_transaction) | list -%}
  {%- if selected and (inside_transaction or adapter.behavior.use_non_transactional_hooks) -%}
    {%- for hook in selected -%}
      {%- set rendered = render(hook.get('sql')) | trim -%}
      {%- if (rendered | length) > 0 -%}
        {%- call statement(auto_begin=inside_transaction) -%}
          {{ rendered }}
        {%- endcall -%}
      {%- endif -%}
    {%- endfor -%}
  {%- endif -%}
{% endmacro %}