{% set res, tab = adapter.execute("select 1 as one") %}

{{ print(res) }}
{{ print(tab) }}

SELECT 1 as one
