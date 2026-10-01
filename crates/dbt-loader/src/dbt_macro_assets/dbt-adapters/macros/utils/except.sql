{% macro except() %}
  {{ return(adapter.dispatch('except', 'dbt')()) }}
{% endmacro %}

{% macro default__except() %}

    except

{% endmacro %}
