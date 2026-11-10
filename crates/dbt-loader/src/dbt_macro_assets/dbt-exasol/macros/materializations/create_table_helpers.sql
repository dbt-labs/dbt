{% macro exasol__post_create_table_alters(relation, contract_config) %}
    {#- Contract constraints (column-level NOT NULL, model-level PRIMARY KEY)
        and the Exasol table-tuning configs, emitted as one ALTER per action
        after the CTAS. UNIQUE/CHECK are rejected by get_constraint_support.
        A contract primary_key constraint takes precedence over the
        primary_key_config table config (a table can have only one PK). -#}
    {%- set has_contract_pk = namespace(value=false) -%}

    {%- if contract_config.enforced -%}
        {%- for col_name in model.columns -%}
            {%- set col = model.columns[col_name] -%}
            {%- for constraint in col.constraints -%}
                {%- if constraint.type == 'not_null' -%};
    alter table {{ relation }} modify column {{ adapter.quote(col.name) if col.quote else col.name }} not null
                {%- endif -%}
            {%- endfor -%}
        {%- endfor -%}
        {%- for constraint in model.constraints -%}
            {%- if constraint.type == 'primary_key' and constraint.columns -%}
                {%- set has_contract_pk.value = true -%};
    alter table {{ relation }} add constraint {{ adapter.quote((relation.identifier | replace('__dbt_tmp', '')) ~ '__pk') }} primary key ({{ constraint.columns | join(', ') }})
            {%- endif -%}
        {%- endfor -%}
    {%- endif -%}

    {%- set partition_by_config = config.get('partition_by_config') -%}
    {%- set distribute_by_config = config.get('distribute_by_config') -%}
    {%- set primary_key_config = config.get('primary_key_config') -%}

    {%- if distribute_by_config is not none -%}
        {%- set cols = [distribute_by_config] if distribute_by_config is string else distribute_by_config -%};
    alter table {{ relation }} distribute by {{ cols | join(', ') }}
    {%- endif -%}
    {%- if partition_by_config is not none -%}
        {%- set cols = [partition_by_config] if partition_by_config is string else partition_by_config -%};
    alter table {{ relation }} partition by {{ cols | join(', ') }}
    {%- endif -%}
    {%- if primary_key_config is not none and not has_contract_pk.value -%}
        {%- set cols = [primary_key_config] if primary_key_config is string else primary_key_config -%};
    alter table {{ relation }} add constraint {{ adapter.quote((relation.identifier | replace('__dbt_tmp', '')) ~ '__pk') }} primary key ({{ cols | join(', ') }})
    {%- endif -%}
{% endmacro %}
