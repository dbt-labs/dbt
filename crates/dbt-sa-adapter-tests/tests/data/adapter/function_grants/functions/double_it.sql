{%- if target.type == 'redshift' -%}
select $1 * 2
{%- else -%}
x * 2
{%- endif -%}
