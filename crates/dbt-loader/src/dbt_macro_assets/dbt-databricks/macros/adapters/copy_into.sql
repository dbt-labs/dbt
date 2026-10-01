{% macro databricks_copy_into(
  target_table,
  source,
  file_format,
  expression_list=none,
  source_credential=none,
  source_encryption=none,
  validate=none,
  files=none,
  pattern=none,
  format_options=none,
  copy_options=none) -%}

  {% set target_relation_exists, target_relation = get_or_create_relation(
        database=target.database,
        schema=target.schema,
        identifier=target_table,
        type='table') -%}

  {%- set source_clause -%}
    {%- if expression_list -%}
      ( select {{ expression_list }} from '{{ source }}' )
    {%- else -%}
      '{{ source }}'
    {%- endif -%}
    {%- if source_credential or source_encryption %}
      WITH (
      {%- if source_credential %}
        credential (
          {%- for name in source_credential -%}
            '{{ name }}' = '{{ source_credential[name] }}' {%- if not loop.last %}, {% endif -%}
          {%- endfor -%}
        )
      {%- endif %}
      {%- if source_encryption %}
        encryption (
          {%- for name in source_encryption -%}
            '{{ name }}' = '{{ source_encryption[name] }}' {%- if not loop.last %}, {% endif -%}
          {%- endfor -%}
        )
      {%- endif %}
      )
    {%- endif -%}
  {%- endset -%}

  {% set query %}
    copy into {{ target_relation }}
    from {{ source_clause }}
    fileformat = {{ file_format }}
    {% if validate -%} validate {{ validate }} {%- endif %}
    {% if files and pattern %}
        {{ exceptions.raise_compiler_error("You can only specify one of 'files' or 'pattern'") }}
    {% endif %}
    {% if files -%}
      files = (
        {%- for file in files -%}
          '{{ file }}' {%- if not loop.last %}, {% endif -%}
        {%- endfor -%}
      )
    {%- endif %}
    {% if pattern -%}
        pattern = '{{ pattern }}'
    {%- endif %}
    {% if format_options -%}
      format_options (
        {%- for key in format_options -%}
          '{{ key }}' = '{{ format_options[key] }}' {%- if not loop.last %}, {% endif -%}
        {%- endfor -%}
      )
    {%- endif %}
    {% if copy_options -%}
      copy_options (
        {%- for key in copy_options -%}
          '{{ key }}' = '{{ copy_options[key] }}' {%- if not loop.last %}, {% endif -%}
        {%- endfor -%}
      )
    {%- endif %}
  {% endset %}

  {{ run_query_as(query, 'copy_into', fetch_result=False) }}
{% endmacro %}
