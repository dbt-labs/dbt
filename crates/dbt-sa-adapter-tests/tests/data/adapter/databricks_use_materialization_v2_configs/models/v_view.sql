{{ config(materialized='view', view_update_via_alter=true, tblproperties={'test_property': 'test_value'}) }}
select 1 as id, 'a' as val
