-- funcsign: (relation, string) -> relation
{% macro salesforce__make_intermediate_relation(base_relation, suffix) %}
    {# TODO: this is probably not supported for salesforce, but maybe we do fall back to mk_temp_relation... #}
    {% do exceptions.raise_not_implemented('make_intermediate_relation macro not implemented for adapter ' ~ adapter.type()) %}

    {{ return(salesforce__make_temp_relation(base_relation, suffix)) }}
{% endmacro %}

-- funcsign: (relation, string) -> relation
{% macro salesforce__make_temp_relation(base_relation, suffix) %}
    {# TODO: we can't simply just suffix the identifier since it needs to end with the `__dll` suffix #}
    {% do exceptions.raise_not_implemented('make_temp_relation macro not implemented for adapter ' ~ adapter.type()) %}

    {%- set temp_identifier = base_relation.identifier ~ suffix -%}
    {%- set temp_relation = base_relation.incorporate(
                                path={"identifier": temp_identifier}) -%}

    {{ return(temp_relation) }}
{% endmacro %}

-- funcsign: (relation, string, string) -> relation
{% macro salesforce__make_backup_relation(base_relation, backup_relation_type, suffix) %}
    {# TODO: this is probably not supported for salesforce #}
    {% do exceptions.raise_not_implemented('make_backup_relation macro not implemented for adapter ' ~ adapter.type()) %}

    {%- set backup_identifier = base_relation.identifier ~ suffix -%}
    {%- set backup_relation = base_relation.incorporate(
                                  path={"identifier": backup_identifier},
                                  type=backup_relation_type
    ) -%}
    {{ return(backup_relation) }}
{% endmacro %}

-- funcsign: (relation) -> string
{% macro salesforce__truncate_relation(relation) -%}
    {# TODO: this is probably not supported for salesforce #}
    {% do exceptions.raise_not_implemented('truncate_relation macro not implemented for adapter ' ~ adapter.type()) %}
{% endmacro %}
