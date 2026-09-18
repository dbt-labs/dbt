import json
import re
from datetime import date, datetime
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Union

import jsonschema
from jsonschema import ValidationError
from jsonschema._keywords import type as type_rule
from jsonschema.validators import Draft7Validator, extend

from dbt import deprecations
from dbt.jsonschemas import JSONSCHEMAS_PATH
from dbt_common.context import get_invocation_context

_PROJECT_SCHEMA: Optional[Dict[str, Any]] = None
_RESOURCES_SCHEMA: Optional[Dict[str, Any]] = None

_JSONSCHEMA_SUPPORTED_ADAPTERS = {
    "bigquery",
    "databricks",
    "redshift",
    "snowflake",
}

_HIERARCHICAL_CONFIG_KEYS = {
    "seeds",
    "sources",
    "models",
    "snapshots",
    "tests",
    "exposures",
    "data_tests",
    "metrics",
    "saved_queries",
    "semantic_models",
    "unit_tests",
}

_ADAPTER_TO_EXTRA_KEYS = {
    "bigquery": ["dataset", "project"],
    # databricks/dbt-databricks#1540, #1302
    "databricks": [
        "query_tags",
        "zorder",
        "options",
        "unique_tmp_table_suffix",
        "skip_optimize",
    ],
}


def load_json_from_package(jsonschema_type: str, filename: str) -> Dict[str, Any]:
    """Loads a JSON file from within a package."""

    path = Path(JSONSCHEMAS_PATH).joinpath(jsonschema_type, filename)
    data = path.read_bytes()
    return json.loads(data)


def project_schema() -> Dict[str, Any]:
    global _PROJECT_SCHEMA

    if _PROJECT_SCHEMA is None:
        _PROJECT_SCHEMA = load_json_from_package(jsonschema_type="project", filename="latest.json")
    return _PROJECT_SCHEMA


def resources_schema() -> Dict[str, Any]:
    global _RESOURCES_SCHEMA

    if _RESOURCES_SCHEMA is None:
        _RESOURCES_SCHEMA = load_json_from_package(
            jsonschema_type="resources", filename="latest.json"
        )

    return _RESOURCES_SCHEMA


def custom_type_rule(validator, types, instance, schema):
    """This is necessary because PyYAML loads things that look like dates or datetimes as those
    python objects. Then jsonschema.validate() fails because it expects strings.
    """
    if "string" in types and (isinstance(instance, datetime) or isinstance(instance, date)):
        return
    else:
        return type_rule(validator, types, instance, schema)


CustomDraft7Validator = extend(Draft7Validator, validators={"type": custom_type_rule})


def error_path_to_string(error: jsonschema.ValidationError) -> str:
    if len(error.path) == 0:
        return ""
    else:
        path = str(error.path.popleft())
        for part in error.path:
            if isinstance(part, int):
                path += f"[{part}]"
            else:
                path += f".{part}"

        return path


def _additional_properties_violation_keys(error: ValidationError) -> List[str]:
    found_keys = re.findall(r"'\S+'", error.message)
    return [key.strip("'") for key in found_keys]


def _validate_with_schema(
    schema: Dict[str, Any], json: Dict[str, Any]
) -> Iterator[ValidationError]:
    validator = CustomDraft7Validator(schema)
    return validator.iter_errors(json)


def _get_allowed_config_extra_keys() -> List[str]:
    extra_keys = []
    invocation_context = get_invocation_context()
    for adapter in invocation_context.adapter_types:
        if adapter in _ADAPTER_TO_EXTRA_KEYS:
            extra_keys.extend(_ADAPTER_TO_EXTRA_KEYS[adapter])

    return extra_keys


def _get_allowed_config_fields_for_project_property(schema, property_field_name) -> List[str]:
    property_defn = schema["properties"].get(property_field_name)
    property_defn_name = None
    if property_defn and "anyOf" in property_defn:
        for any_of_item in property_defn["anyOf"]:
            if "$ref" in any_of_item:
                property_defn_name = any_of_item["$ref"].split("/")[-1]
                break

    if property_defn_name is None:
        return []

    allowed_config_fields = set(schema["definitions"][property_defn_name]["properties"])
    # in dbt_project.yml keys should have a + prefix
    allowed_config_fields.update([f"+{key}" for key in _get_allowed_config_extra_keys()])
    return list(allowed_config_fields)


def _get_allowed_config_fields_from_error_path(
    yml_schema: Dict[str, Any], error_path: List[Union[str, int]]
) -> Optional[List[str]]:
    property_field_name = None
    node_schema = yml_schema["properties"]
    for part in error_path:
        if isinstance(part, str):
            if part in node_schema:
                if "items" not in node_schema[part]:
                    break

                # Update property field name
                property_field_name = node_schema[part]["items"]["$ref"].split("/")[-1]

                # Jump to the next level of the schema
                item_definition = node_schema[part]["items"]["$ref"].split("/")[-1]
                node_schema = yml_schema["definitions"][item_definition]["properties"]

    if not property_field_name:
        return None

    if "config" not in yml_schema["definitions"][property_field_name]["properties"]:
        return None

    config_field_name = yml_schema["definitions"][property_field_name]["properties"]["config"][
        "anyOf"
    ][0]["$ref"].split("/")[-1]

    allowed_config_fields = list(set(yml_schema["definitions"][config_field_name]["properties"]))
    allowed_config_fields.extend(_get_allowed_config_extra_keys())

    return allowed_config_fields


def _can_run_validations() -> bool:
    invocation_context = get_invocation_context()
    # for commands like dbt deps, we don't have adapter types set yet
    if not invocation_context.adapter_types:
        return False
    return invocation_context.adapter_types.issubset(_JSONSCHEMA_SUPPORTED_ADAPTERS)


def jsonschema_validate(schema: Dict[str, Any], json: Dict[str, Any], file_path: str) -> None:
    if not _can_run_validations():
        return

    errors = _validate_with_schema(schema, json)
    for error in errors:
        # Listify the error path to make it easier to work with (it's a deque in the ValidationError object)
        error_path = list(error.path)
        if error.validator == "additionalProperties":
            keys = _additional_properties_violation_keys(error)
            if len(error.path) == 0:
                for key in keys:
                    deprecations.warn(
                        "custom-top-level-key-deprecation",
                        msg="Unexpected top-level key" + (" " + key if key else ""),
                        file=file_path,
                    )
            else:
                key_path = error_path_to_string(error)
                for key in keys:
                    # Type params are not in the metrics v2 jsonschema from the v2 parser, but dbt-core continues to maintain support for them in v1.
                    if key == "type_params":
                        continue

                    # 'dataset' and 'project' are valid top-level source properties for BigQuery
                    if (
                        len(error_path) == 2
                        and error_path[0] == "sources"
                        and isinstance(error_path[1], int)
                        and key in _get_allowed_config_extra_keys()
                    ):
                        continue

                    if key == "overrides" and key_path.startswith("sources"):
                        deprecations.warn(
                            "source-override-deprecation",
                            source_name=key_path.split(".")[-1],
                            file=file_path,
                        )
                    else:
                        allowed_config_fields = _get_allowed_config_fields_from_error_path(
                            schema, error_path
                        )
                        if allowed_config_fields and key in allowed_config_fields:
                            deprecations.warn(
                                "property-moved-to-config-deprecation",
                                key=key,
                                file=file_path,
                                key_path=key_path,
                            )
                        else:
                            deprecations.warn(
                                "custom-key-in-object-deprecation",
                                key=key,
                                file=file_path,
                                key_path=key_path,
                            )
        elif error.validator == "anyOf" and len(error_path) > 0:
            sub_errors = error.context or []
            # schema yaml resource configs
            if error_path[-1] == "config":
                for sub_error in sub_errors:
                    if (
                        isinstance(sub_error, ValidationError)
                        and sub_error.validator == "additionalProperties"
                    ):
                        keys = _additional_properties_violation_keys(sub_error)
                        key_path = error_path_to_string(error)
                        for key in keys:
                            if key in _get_allowed_config_extra_keys():
                                continue

                            deprecations.warn(
                                "custom-key-in-config-deprecation",
                                key=key,
                                file=file_path,
                                key_path=key_path,
                            )
            # dbt_project.yml configs
            elif "dbt_project.yml" in file_path and error_path[0] in _HIERARCHICAL_CONFIG_KEYS:
                allowed_config_fields = _get_allowed_config_fields_for_project_property(
                    schema, property_field_name=error_path[0]
                )
                for sub_error in sub_errors:
                    if not isinstance(sub_error, ValidationError) or sub_error.validator != "type":
                        continue
                    if not sub_error.path or not isinstance(sub_error.path[-1], str):
                        continue

                    key = sub_error.path[-1]
                    had_valid_config_key_in_path = any(
                        k in allowed_config_fields for k in sub_error.path
                    )
                    if f"+{key}" in allowed_config_fields and not had_valid_config_key_in_path:
                        deprecations.warn(
                            "missing-plus-prefix-in-config-deprecation",
                            key=key,
                            file=file_path,
                            key_path=error_path_to_string(sub_error),
                        )
                    elif key not in allowed_config_fields:
                        deprecations.warn(
                            "custom-key-in-config-deprecation",
                            key=key,
                            file=file_path,
                            key_path=error_path_to_string(sub_error),
                        )
        elif error.validator == "type":
            # Not deprecating invalid types yet
            pass
        else:
            deprecations.warn(
                "generic-json-schema-validation-deprecation",
                violation=error.message,
                file=file_path,
                key_path=error_path_to_string(error),
            )


def validate_model_config(
    config: Dict[str, Any], file_path: str, is_python_model: bool = False
) -> None:
    if not _can_run_validations():
        return

    resources_jsonschema = resources_schema()
    nested_definition_name = "ModelConfig"

    model_config_schema = {
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": nested_definition_name,
        **resources_jsonschema["definitions"][nested_definition_name],
        "definitions": {
            k: v
            for k, v in resources_jsonschema["definitions"].items()
            if k != nested_definition_name
        },
    }

    errors = _validate_with_schema(model_config_schema, config)
    for error in errors:
        error_path = list(error.path)
        if error.validator == "additionalProperties":
            keys = _additional_properties_violation_keys(error)
            if len(error.path) == 0:
                key_path = error_path_to_string(error)
                for key in keys:
                    # Special case for pre/post hook keys as they are updated during config parsing
                    # from the user-provided pre_hook/post_hook to pre-hook/post-hook keys.
                    # Avoids false positives as described in https://github.com/dbt-labs/dbt-core/issues/12087
                    if key in ("post-hook", "pre-hook"):
                        continue

                    # Special case for python model internal key additions
                    # These keys are added during python model parsing and are not user-provided
                    python_model_internal_keys = (
                        "config_keys_used",
                        "config_keys_defaults",
                        "meta_keys_used",
                        "meta_keys_defaults",
                    )
                    if key in python_model_internal_keys and is_python_model:
                        continue

                    # Dont raise deprecation warnings for adapter specific config key aliases
                    if key in _get_allowed_config_extra_keys():
                        continue

                    # For everything else, emit deprecation warning
                    deprecations.warn(
                        "custom-key-in-config-deprecation",
                        key=key,
                        file=file_path,
                        key_path=key_path,
                    )
            else:
                error.path.appendleft("config")
                key_path = error_path_to_string(error)
                for key in keys:
                    deprecations.warn(
                        "custom-key-in-object-deprecation",
                        key=key,
                        file=file_path,
                        key_path=key_path,
                    )
        elif error.validator == "type":
            # Not deprecating invalid types yet, except for pre-existing deprecation_date deprecation
            pass
        elif error.validator == "anyOf" and len(error_path) > 0:
            for sub_error in error.context or []:
                if (
                    isinstance(sub_error, ValidationError)
                    and sub_error.validator == "additionalProperties"
                ):
                    error.path.appendleft("config")
                    keys = _additional_properties_violation_keys(sub_error)
                    key_path = error_path_to_string(error)
                    for key in keys:
                        deprecations.warn(
                            "custom-key-in-object-deprecation",
                            key=key,
                            file=file_path,
                            key_path=key_path,
                        )
        else:
            deprecations.warn(
                "generic-json-schema-validation-deprecation",
                violation=error.message,
                file=file_path,
                key_path=error_path_to_string(error),
            )
