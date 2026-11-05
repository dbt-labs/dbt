import json
import os

MANIFEST_SCHEMA_PATH = os.path.join(
    os.path.dirname(__file__), "..", "..", "..", "schemas", "dbt", "manifest", "v12.json"
)


def test_manifest_schema_declares_exposure_dimensions():
    with open(MANIFEST_SCHEMA_PATH) as f:
        schema = json.load(f)
    exposure_schema = schema["properties"]["exposures"]["additionalProperties"]
    assert exposure_schema["properties"]["dimensions"] == {
        "type": "array",
        "items": {"type": "string"},
    }
