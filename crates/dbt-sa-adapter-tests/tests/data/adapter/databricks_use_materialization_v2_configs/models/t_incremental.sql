{{ config(materialized='incremental', incremental_strategy='append', incremental_apply_config_changes=true, tblproperties={'test_property': 'test_value'}) }}
select 1 as id
