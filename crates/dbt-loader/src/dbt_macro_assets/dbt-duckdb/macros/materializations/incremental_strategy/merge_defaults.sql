{% macro merge_clause_defaults(
    merge_update_condition,
    merge_insert_condition,
    merge_update_columns=[],
    merge_exclude_columns=[],
    merge_update_set_expressions={}
) -%}

    {{ return({
        'when_matched_update_by_name': {
            'action': 'update',
            'condition': merge_update_condition,
            'mode': 'by_name'
        },
        'when_not_matched_insert_by_name': {
            'action': 'insert',
            'condition': merge_insert_condition,
            'mode': 'by_name'
        },
        'when_matched_update_explicit': {
            'action': 'update',
            'condition': merge_update_condition,
            'mode': 'explicit',
            'update': {
                'include': merge_update_columns,
                'exclude': merge_exclude_columns,
                'set_expressions': merge_update_set_expressions
            }
        }
    }) }}
{%- endmacro %}
