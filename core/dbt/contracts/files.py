import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Union

from mashumaro.types import SerializableType

from dbt.artifacts.resources.base import FileHash
from dbt.constants import MAXIMUM_SEED_SIZE
from dbt_common.dataclass_schema import StrEnum, dbtClassMixin

from .util import SourceKey


class ParseFileType(StrEnum):
    Macro = "macro"
    Model = "model"
    Snapshot = "snapshot"
    Analysis = "analysis"
    SingularTest = "singular_test"
    GenericTest = "generic_test"
    Seed = "seed"
    Documentation = "docs"
    Schema = "schema"
    Hook = "hook"  # not a real filetype, from dbt_project.yml
    Fixture = "fixture"


parse_file_type_to_parser = {
    ParseFileType.Macro: "MacroParser",
    ParseFileType.Model: "ModelParser",
    ParseFileType.Snapshot: "SnapshotParser",
    ParseFileType.Analysis: "AnalysisParser",
    ParseFileType.SingularTest: "SingularTestParser",
    ParseFileType.GenericTest: "GenericTestParser",
    ParseFileType.Seed: "SeedParser",
    ParseFileType.Documentation: "DocumentationParser",
    ParseFileType.Schema: "SchemaParser",
    ParseFileType.Hook: "HookParser",
    ParseFileType.Fixture: "FixtureParser",
}


@dataclass
class FilePath(dbtClassMixin):
    searched_path: str
    relative_path: str
    modification_time: float
    project_root: str

    @property
    def search_key(self) -> str:
        # TODO: should this be project name + path relative to project root?
        return self.absolute_path

    @property
    def full_path(self) -> str:
        # useful for symlink preservation
        return os.path.join(self.project_root, self.searched_path, self.relative_path)

    @property
    def absolute_path(self) -> str:
        return os.path.abspath(self.full_path)

    @property
    def original_file_path(self) -> str:
        return os.path.join(self.searched_path, self.relative_path)

    def seed_too_large(self) -> bool:
        """Return whether the file this represents is over the seed size limit"""
        return os.stat(self.full_path).st_size > MAXIMUM_SEED_SIZE


@dataclass
class RemoteFile(dbtClassMixin):
    def __init__(self, language) -> None:
        if language == "sql":
            self.path_end = ".sql"
        elif language == "python":
            self.path_end = ".py"
        else:
            raise RuntimeError(f"Invalid language for remote File {language}")
        self.path = f"from remote system{self.path_end}"

    @property
    def searched_path(self) -> str:
        return self.path

    @property
    def relative_path(self) -> str:
        return self.path

    @property
    def absolute_path(self) -> str:
        return self.path

    @property
    def original_file_path(self):
        return self.path

    @property
    def modification_time(self):
        return self.path


@dataclass
class BaseSourceFile(dbtClassMixin, SerializableType):
    """Define a source file in dbt"""

    path: Union[FilePath, RemoteFile]  # the path information
    checksum: FileHash
    # Seems like knowing which project the file came from would be useful
    project_name: Optional[str] = None
    # Parse file type: i.e. which parser will process this file
    parse_file_type: Optional[ParseFileType] = None
    # we don't want to serialize this
    contents: Optional[str] = None

    @property
    def file_id(self):
        if isinstance(self.path, RemoteFile):
            return None
        return f"{self.project_name}://{self.path.original_file_path}"

    @property
    def original_file_path(self):
        return self.path.original_file_path

    def _serialize(self):
        dct = self.to_dict()
        return dct

    @classmethod
    def _deserialize(cls, dct: Dict[str, int]):
        if dct["parse_file_type"] == "schema":
            sf = SchemaSourceFile.from_dict(dct)
        elif dct["parse_file_type"] == "fixture":
            sf = FixtureSourceFile.from_dict(dct)
        else:
            sf = SourceFile.from_dict(dct)
        return sf

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        # remove empty lists to save space
        dct_keys = list(dct.keys())
        for key in dct_keys:
            if isinstance(dct[key], list) and not dct[key]:
                del dct[key]
        # remove contents. Schema files will still have 'dict_from_yaml'
        # from the contents
        if "contents" in dct:
            del dct["contents"]
        return dct


@dataclass
class SourceFile(BaseSourceFile):
    nodes: List[str] = field(default_factory=list)
    docs: List[str] = field(default_factory=list)
    macros: List[str] = field(default_factory=list)
    env_vars: List[str] = field(default_factory=list)

    @classmethod
    def big_seed(cls, path: FilePath) -> "SourceFile":
        """Parse seeds over the size limit with just the path"""
        self = cls(path=path, checksum=FileHash.path(path.original_file_path))
        self.contents = ""
        return self

    def add_node(self, value):
        if value not in self.nodes:
            self.nodes.append(value)

    # TODO: do this a different way. This remote file kludge isn't going
    # to work long term
    @classmethod
    def remote(cls, contents: str, project_name: str, language: str) -> "SourceFile":
        self = cls(
            path=RemoteFile(language),
            checksum=FileHash.from_contents(contents),
            project_name=project_name,
            contents=contents,
        )
        return self


@dataclass
class SchemaSourceFile(BaseSourceFile):
    dfy: Dict[str, Any] = field(default_factory=dict)
    # these are in the manifest.nodes dictionary
    data_tests: Dict[str, Any] = field(default_factory=dict)
    sources: List[str] = field(default_factory=list)
    exposures: List[str] = field(default_factory=list)
    metrics: List[str] = field(default_factory=list)
    snapshots: List[str] = field(default_factory=list)
    # The following field will no longer be used. Leaving
    # here to avoid breaking existing projects. To be removed
    # later if possible.
    generated_metrics: List[str] = field(default_factory=list)
    # metrics generated from semantic_model measures. The key is
    # the name of the semantic_model, so that we can find it later.
    metrics_from_measures: Dict[str, Any] = field(default_factory=dict)
    groups: List[str] = field(default_factory=list)
    # node patches contain models, seeds, snapshots, analyses
    ndp: List[str] = field(default_factory=list)
    semantic_models: List[str] = field(default_factory=list)
    unit_tests: List[str] = field(default_factory=list)
    saved_queries: List[str] = field(default_factory=list)
    # any macro patches in this file by macro unique_id.
    mcp: Dict[str, str] = field(default_factory=dict)
    # any source patches in this file. The entries are package, name pairs
    # Patches are only against external sources. Sources can be
    # created too, but those are in 'sources'
    sop: List[SourceKey] = field(default_factory=list)
    env_vars: Dict[str, Any] = field(default_factory=dict)
    unrendered_configs: Dict[str, Any] = field(default_factory=dict)
    unrendered_databases: Dict[str, Any] = field(default_factory=dict)
    unrendered_schemas: Dict[str, Any] = field(default_factory=dict)
    vars: Dict[str, Any] = field(default_factory=dict)
    pp_dict: Optional[Dict[str, Any]] = None
    pp_test_index: Optional[Dict[str, Any]] = None

    @property
    def dict_from_yaml(self):
        return self.dfy

    @property
    def node_patches(self):
        return self.ndp

    @property
    def macro_patches(self):
        return self.mcp

    @property
    def source_patches(self):
        return self.sop

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        # Remove partial parsing specific data
        for key in ("pp_test_index", "pp_dict"):
            if key in dct:
                del dct[key]
        return dct

    def append_patch(self, yaml_key, unique_id):
        self.node_patches.append(unique_id)

    def add_test(self, node_unique_id, test_from):
        name = test_from["name"]
        key = test_from["key"]
        if key not in self.data_tests:
            self.data_tests[key] = {}
        if name not in self.data_tests[key]:
            self.data_tests[key][name] = []
        self.data_tests[key][name].append(node_unique_id)

    # this is only used in tests/unit
    def remove_tests(self, yaml_key, name):
        if yaml_key in self.data_tests:
            if name in self.data_tests[yaml_key]:
                del self.data_tests[yaml_key][name]

    # this is only used in the tests directory (unit + functional)
    def get_tests(self, yaml_key, name):
        if yaml_key in self.data_tests:
            if name in self.data_tests[yaml_key]:
                return self.data_tests[yaml_key][name]
        return []

    def add_metrics_from_measures(self, semantic_model_name: str, metric_unique_id: str):
        if self.generated_metrics:
            # Probably not needed, but for safety sake, convert the
            # old generated_metrics to metrics_from_measures.
            self.fix_metrics_from_measures()
        if semantic_model_name not in self.metrics_from_measures:
            self.metrics_from_measures[semantic_model_name] = []
        self.metrics_from_measures[semantic_model_name].append(metric_unique_id)

    def fix_metrics_from_measures(self):
        # Temporary method to fix up existing projects with a partial parse file.
        # This should only be called if SchemaSourceFile in a msgpack
        # pack manifest has an existing "generated_metrics" list, to turn it
        # it into a "metrics_from_measures" dictionary, so that we can
        # correctly partially parse.
        # This code can be removed when "generated_metrics" is removed.
        generated_metrics = self.generated_metrics
        self.generated_metrics = []  # Should never be needed again
        # For each metric_unique_id we loop through the semantic models
        # looking for the name of the "measure" which generated the metric.
        # When it's found, add it to "metrics_from_measures", with a key
        # of the semantic_model name, and a list of metrics.
        for metric_unique_id in generated_metrics:
            parts = metric_unique_id.split(".")
            # get the metric_name
            metric_name = parts[-1]
            if "semantic_models" in self.dict_from_yaml:
                for sem_model in self.dict_from_yaml["semantic_models"]:
                    if "measures" in sem_model:
                        for measure in sem_model["measures"]:
                            if measure["name"] == metric_name:
                                self.add_metrics_from_measures(sem_model["name"], metric_unique_id)
                                break

    def get_key_and_name_for_test(self, test_unique_id):
        yaml_key = None
        block_name = None
        for key in self.data_tests.keys():
            for name in self.data_tests[key]:
                for unique_id in self.data_tests[key][name]:
                    if unique_id == test_unique_id:
                        yaml_key = key
                        block_name = name
                        break
        return (yaml_key, block_name)

    def get_all_test_ids(self):
        test_ids = []
        for key in self.data_tests.keys():
            for name in self.data_tests[key]:
                test_ids.extend(self.data_tests[key][name])
        return test_ids

    def add_unrendered_config(self, unrendered_config, yaml_key, name, version=None):
        versioned_name = f"{name}_v{version}" if version is not None else name

        if yaml_key not in self.unrendered_configs:
            self.unrendered_configs[yaml_key] = {}

        if versioned_name not in self.unrendered_configs[yaml_key]:
            self.unrendered_configs[yaml_key][versioned_name] = unrendered_config

    def get_unrendered_config(self, yaml_key, name, version=None) -> Optional[Dict[str, Any]]:
        versioned_name = f"{name}_v{version}" if version is not None else name

        if yaml_key not in self.unrendered_configs:
            return None
        if versioned_name not in self.unrendered_configs[yaml_key]:
            return None

        return self.unrendered_configs[yaml_key][versioned_name]

    def delete_from_unrendered_configs(self, yaml_key, name):
        # We delete all unrendered_configs for this yaml_key/name because the
        # entry has been scheduled for reparsing.
        if self.get_unrendered_config(yaml_key, name):
            del self.unrendered_configs[yaml_key][name]
            # Delete all versioned keys associated with name
            version_names_to_delete = []
            for potential_version_name in self.unrendered_configs[yaml_key]:
                if potential_version_name.startswith(f"{name}_v"):
                    version_names_to_delete.append(potential_version_name)
            for version_name in version_names_to_delete:
                del self.unrendered_configs[yaml_key][version_name]

            if not self.unrendered_configs[yaml_key]:
                del self.unrendered_configs[yaml_key]

    def add_vars(self, vars: Dict[str, Any], yaml_key: str, name: str) -> None:
        if yaml_key not in self.vars:
            self.vars[yaml_key] = {}

        if name not in self.vars[yaml_key]:
            self.vars[yaml_key][name] = vars

    def get_vars(self, yaml_key: str, name: str) -> Dict[str, Any]:
        if yaml_key not in self.vars:
            return {}

        if name not in self.vars[yaml_key]:
            return {}

        return self.vars[yaml_key][name]

    def add_env_var(self, var, yaml_key, name):
        if yaml_key not in self.env_vars:
            self.env_vars[yaml_key] = {}
        if name not in self.env_vars[yaml_key]:
            self.env_vars[yaml_key][name] = []
        if var not in self.env_vars[yaml_key][name]:
            self.env_vars[yaml_key][name].append(var)

    def delete_from_env_vars(self, yaml_key, name):
        # We delete all vars for this yaml_key/name because the
        # entry has been scheduled for reparsing.
        if yaml_key in self.env_vars and name in self.env_vars[yaml_key]:
            del self.env_vars[yaml_key][name]
            if not self.env_vars[yaml_key]:
                del self.env_vars[yaml_key]

    def add_unrendered_database(self, yaml_key: str, name: str, unrendered_database: str) -> None:
        if yaml_key not in self.unrendered_databases:
            self.unrendered_databases[yaml_key] = {}

        self.unrendered_databases[yaml_key][name] = unrendered_database

    def get_unrendered_database(self, yaml_key: str, name: str) -> Optional[str]:
        if yaml_key not in self.unrendered_databases:
            return None

        return self.unrendered_databases[yaml_key].get(name)

    def add_unrendered_schema(self, yaml_key: str, name: str, unrendered_schema: str) -> None:
        if yaml_key not in self.unrendered_schemas:
            self.unrendered_schemas[yaml_key] = {}

        self.unrendered_schemas[yaml_key][name] = unrendered_schema

    def get_unrendered_schema(self, yaml_key: str, name: str) -> Optional[str]:
        if yaml_key not in self.unrendered_schemas:
            return None

        return self.unrendered_schemas[yaml_key].get(name)


@dataclass
class FixtureSourceFile(BaseSourceFile):
    fixture: Optional[str] = None
    unit_tests: List[str] = field(default_factory=list)

    def add_unit_test(self, value):
        if value not in self.unit_tests:
            self.unit_tests.append(value)


AnySourceFile = Union[SchemaSourceFile, SourceFile, FixtureSourceFile]
