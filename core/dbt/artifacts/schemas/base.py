import dataclasses
from datetime import datetime
import functools
from mashumaro.jsonschema import build_json_schema
from mashumaro.jsonschema.dialects import DRAFT_2020_12
from typing import ClassVar, Type, TypeVar, Dict, Any, Optional

from dbt_common.clients.system import write_json, read_json
from dbt_common.exceptions import DbtInternalError, DbtRuntimeError
from dbt_common.events.functions import get_metadata_vars
from dbt_common.invocation import get_invocation_id
from dbt_common.dataclass_schema import dbtClassMixin

from dbt.version import __version__
from dbt.artifacts.exceptions import IncompatibleSchemaError


BASE_SCHEMAS_URL = "https://schemas.getdbt.com/"
SCHEMA_PATH = "dbt/{name}/v{version}.json"


@dataclasses.dataclass
class SchemaVersion:
    name: str
    version: int

    @property
    def path(self) -> str:
        return SCHEMA_PATH.format(name=self.name, version=self.version)

    def __str__(self) -> str:
        return BASE_SCHEMAS_URL + self.path


class Writable:
    def write(self, path: str):
        write_json(path, self.to_dict(omit_none=False))  # type: ignore


class Readable:
    @classmethod
    def read(cls, path: str):
        try:
            data = read_json(path)
        except (EnvironmentError, ValueError) as exc:
            raise DbtRuntimeError(
                f'Could not read {cls.__name__} at "{path}" as JSON: {exc}'
            ) from exc

        return cls.from_dict(data)  # type: ignore


# This is used in the ManifestMetadata, RunResultsMetadata, RunOperationResultMetadata,
# FreshnessMetadata, and CatalogMetadata classes
@dataclasses.dataclass
class BaseArtifactMetadata(dbtClassMixin):
    dbt_schema_version: str
    dbt_version: str = __version__
    generated_at: datetime = dataclasses.field(default_factory=datetime.now)
    invocation_id: Optional[str] = dataclasses.field(default_factory=get_invocation_id)
    env: Dict[str, str] = dataclasses.field(default_factory=get_metadata_vars)

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
        if dct["generated_at"] and dct["generated_at"].endswith("+00:00"):
            dct["generated_at"] = dct["generated_at"].replace("+00:00", "") + "Z"
        return dct


# This is used as a class decorator to set the schema_version in the
# 'dbt_schema_version' class attribute. (It's copied into the metadata objects.)
# Name attributes of SchemaVersion in classes with the 'schema_version' decorator:
#   manifest
#   run-results
#   run-operation-result
#   sources
#   catalog
#   remote-compile-result
#   remote-execution-result
#   remote-run-result
def schema_version(name: str, version: int):
    def inner(cls: Type[VersionedSchema]):
        cls.dbt_schema_version = SchemaVersion(
            name=name,
            version=version,
        )
        return cls

    return inner


# This is used in the ArtifactMixin and RemoteResult classes
@dataclasses.dataclass
class VersionedSchema(dbtClassMixin):
    dbt_schema_version: ClassVar[SchemaVersion]

    @classmethod
    @functools.lru_cache
    def json_schema(cls) -> Dict[str, Any]:
        json_schema_obj = build_json_schema(cls, dialect=DRAFT_2020_12, with_dialect_uri=True)
        json_schema = json_schema_obj.to_dict()
        json_schema["$id"] = str(cls.dbt_schema_version)
        return json_schema

    @classmethod
    def is_compatible_version(cls, schema_version):
        compatible_versions = [str(cls.dbt_schema_version)]
        if hasattr(cls, "compatible_previous_versions"):
            for name, version in cls.compatible_previous_versions():
                compatible_versions.append(str(SchemaVersion(name, version)))
        return str(schema_version) in compatible_versions

    @classmethod
    def read_and_check_versions(cls, path: str):
        try:
            data = read_json(path)
        except (EnvironmentError, ValueError) as exc:
            raise DbtRuntimeError(
                f'Could not read {cls.__name__} at "{path}" as JSON: {exc}'
            ) from exc

        # Check metadata version. There is a class variable 'dbt_schema_version', but
        # that doesn't show up in artifacts, where it only exists in the 'metadata'
        # dictionary.
        if hasattr(cls, "dbt_schema_version"):
            if "metadata" in data and "dbt_schema_version" in data["metadata"]:
                previous_schema_version = data["metadata"]["dbt_schema_version"]
                # cls.dbt_schema_version is a SchemaVersion object
                if not cls.is_compatible_version(previous_schema_version):
                    raise IncompatibleSchemaError(
                        expected=str(cls.dbt_schema_version),
                        found=previous_schema_version,
                    )

        return cls.upgrade_schema_version(data)

    @classmethod
    def upgrade_schema_version(cls, data):
        """This will modify the data (dictionary) passed in to match the current
        artifact schema code, if necessary. This is the default method, which
        just returns the instantiated object via from_dict."""
        return cls.from_dict(data)


T = TypeVar("T", bound="ArtifactMixin")


# metadata should really be a Generic[T_M] where T_M is a TypeVar bound to
# BaseArtifactMetadata. Unfortunately this isn't possible due to a mypy issue:
# https://github.com/python/mypy/issues/7520
# This is used in the WritableManifest, RunResultsArtifact, RunOperationResultsArtifact,
# and CatalogArtifact
@dataclasses.dataclass(init=False)
class ArtifactMixin(VersionedSchema, Writable, Readable):
    metadata: BaseArtifactMetadata

    @classmethod
    def validate(cls, data):
        super().validate(data)
        if cls.dbt_schema_version is None:
            raise DbtInternalError("Cannot call from_dict with no schema version!")


def get_artifact_schema_version(dct: dict) -> int:
    schema_version = dct.get("metadata", {}).get("dbt_schema_version", None)
    if not schema_version:
        raise ValueError("Artifact is missing schema version")

    # schema_version is in this format: https://schemas.getdbt.com/dbt/manifest/v10.json
    # What the code below is doing:
    # 1. Split on "/" – v10.json
    # 2. Split on "." – v10
    # 3. Skip first character – 10
    # 4. Convert to int
    # TODO: If this gets more complicated, turn into a regex
    return int(schema_version.split("/")[-1].split(".")[0][1:])
