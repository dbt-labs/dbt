{% snapshot orders_snapshot %}

{{
    config(
        target_schema='snapshots',
        unique_key='id',
        strategy='timestamp',
        updated_at='updated_at',
        file_format='delta'
    )
}}

select * from {{ ref('orders') }}

{% endsnapshot %}

