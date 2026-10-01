{% macro exasol__split_part(string_text, delimiter_text, part_number) -%}
    {#- Exasol has no SPLIT_PART. Slice between the (part_number-1)th and
        part_number'th delimiter found with INSTR. The Nth delimiter is absent for
        the final part, so fall back to one past the end of the string. The N=1
        case is branched in Jinja: INSTR's occurrence argument must be >= 1, so
        emitting instr(..., part_number - 1) for part_number = 1 (occurrence 0)
        would error even inside an untaken CASE branch. part_number is expected to
        be a literal integer >= 1; out-of-range parts yield an empty string. -#}
    case
        when {{ part_number }} < 1 then ''
        when {{ part_number }} > (length({{ string_text }}) - length(replace({{ string_text }}, {{ delimiter_text }}, ''))) / length({{ delimiter_text }}) + 1 then ''
        else substr(
            {{ string_text }},
            {% if part_number == 1 -%}
                1
            {%- else -%}
                instr({{ string_text }}, {{ delimiter_text }}, 1, {{ part_number }} - 1) + length({{ delimiter_text }})
            {%- endif %},
            (case when instr({{ string_text }}, {{ delimiter_text }}, 1, {{ part_number }}) = 0
                then length({{ string_text }}) + 1
                else instr({{ string_text }}, {{ delimiter_text }}, 1, {{ part_number }})
            end)
            - {% if part_number == 1 -%}1{%- else -%}(instr({{ string_text }}, {{ delimiter_text }}, 1, {{ part_number }} - 1) + length({{ delimiter_text }})){%- endif %}
        )
    end
{%- endmacro %}
