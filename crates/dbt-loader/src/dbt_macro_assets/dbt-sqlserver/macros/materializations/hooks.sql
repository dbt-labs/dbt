{% macro run_hooks(hooks, inside_transaction=True) %}
  {% for hook in hooks | selectattr('transaction', 'equalto', inside_transaction)  %}
    {% if not inside_transaction and loop.first %}
      {% call statement(auto_begin=inside_transaction) %}
        {#- guarded, not a bare COMMIT: nothing guarantees a transaction is
            open here. A bare one appeared safe only because some earlier
            statement had auto-begun one (find_references, until relation.sql
            stopped doing that); with @@TRANCOUNT = 0 it raises Msg 3902. -#}
        if @@trancount > 0 commit;
      {% endcall %}
    {% endif %}
    {% set rendered = render(hook.get('sql')) | trim %}
    {% if (rendered | length) > 0 %}
      {% call statement(auto_begin=inside_transaction) %}
        {{ rendered }}
      {% endcall %}
    {% endif %}
  {% endfor %}
{% endmacro %}
