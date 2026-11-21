{% if execute and target.type == 'databricks' %}
    {% set is_cluster = adapter.is_cluster() %}
    {{ log('is_cluster: ' ~ is_cluster, info=True) }}
{% endif %}

select 1 as id

