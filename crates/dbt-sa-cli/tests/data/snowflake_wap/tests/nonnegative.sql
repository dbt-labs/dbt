{{ config(severity=var('audit_severity', 'error')) }}

select * from {{ ref('orders') }} where id < 0
