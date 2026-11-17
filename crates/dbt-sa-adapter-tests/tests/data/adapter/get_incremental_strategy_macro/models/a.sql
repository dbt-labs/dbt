{% set default_fn = adapter.get_incremental_strategy_macro(context, "default") %}
{{ print('default_fn: ' ~ default_fn) }}

{% if execute and target.type == 'bigquery' %}

    {% set append_fn = adapter.get_incremental_strategy_macro(context, "append") %}
    {{ print('append_fn: ' ~ append_fn) }}

    {% set strategy_arg_dict = ({'target_relation': 'target_relation', 'temp_relation': 'temp_relation', 'unique_key': 'unique_key', 'dest_columns': [], 'incremental_predicates': [] }) %}

    {% set result = default_fn(strategy_arg_dict) %}
    {{ print('default_fn call result:\n' ~ result) }}

    {% set result = append_fn(strategy_arg_dict) %}
    {{ print('append_fn call result:\n' ~ result) }}
{% endif %}
