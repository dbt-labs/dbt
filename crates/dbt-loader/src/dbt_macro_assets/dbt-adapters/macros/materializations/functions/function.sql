{# DIVERGENCE #}
{# We check for the execute stage to block this out because the model and model.config #}
{# don't exist at parse time and we get errors from the dispatch call otherwise #}
{% materialization function, default, supported_languages=['sql', 'python', 'javascript'] %}
    {% set existing_relation = load_cached_relation(this) %}
    {% set target_relation = this.incorporate(type=this.Function) %}

    {% if execute %}
        {{ run_hooks(pre_hooks) }}

        {% set function_type_macro = get_function_macro(model.config.type, model.language) %}
        {% set build_sql = function_type_macro(target_relation) %}

        {{ function_execute_build_sql(build_sql, existing_relation, target_relation) }}

        {{ run_hooks(post_hooks) }}
    {% endif %}

    {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
