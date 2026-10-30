
{% set less_than_version = adapter.compare_dbr_version(major=0, minor=1) %}
{{ print("version 0.1 is less than current version. Result: " ~ less_than_version) }}


{% set greater_than_version = adapter.compare_dbr_version(major=10000, minor=1) %}
{{ print("version 10000.1 is greater than current version. Result: " ~ greater_than_version) }}

select 1 as id