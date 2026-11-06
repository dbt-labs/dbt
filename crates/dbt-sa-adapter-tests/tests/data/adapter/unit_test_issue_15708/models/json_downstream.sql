SELECT id, JSON_VALUE(payload_json, '$.segmentCode') AS segment_code
FROM {{ ref('json_source') }}
