from dataclasses import dataclass, fields
from dbt_common.dataclass_schema import dbtClassMixin
from typing import List, Optional
import hashlib

from dbt.artifacts.resources.types import NodeType


@dataclass
class BaseResource(dbtClassMixin):
    name: str
    resource_type: NodeType
    package_name: str
    path: str
    original_file_path: str
    unique_id: str

    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)

        # Support deserializing additional fields in BaseResource for forward
        # compatibility within a major version
        for f in fields(cls):
            # missing fields with defaults are acceptable
            if f.name not in data and f.default:
                data[f.name] = f.default
            # missing optional fields are acceptable
            elif f.name not in data and f.init is False:
                data[f.name] = None
        return data


@dataclass
class GraphResource(BaseResource):
    fqn: List[str]


@dataclass
class FileHash(dbtClassMixin):
    name: str  # the hash type name
    checksum: str  # the hashlib.hash_type().hexdigest() of the file contents

    @classmethod
    def empty(cls):
        return FileHash(name="none", checksum="")

    @classmethod
    def path(cls, path: str):
        return FileHash(name="path", checksum=path)

    def __eq__(self, other):
        if not isinstance(other, FileHash):
            return NotImplemented

        if self.name == "none" or self.name != other.name:
            return False

        return self.checksum == other.checksum

    def compare(self, contents: str) -> bool:
        """Compare the file contents with the given hash"""
        if self.name == "none":
            return False

        return self.from_contents(contents, name=self.name) == self.checksum

    @classmethod
    def from_contents(cls, contents: str, name="sha256") -> "FileHash":
        """Create a file hash from the given file contents. The hash is always
        the utf-8 encoding of the contents given, because dbt only reads files
        as utf-8.
        """
        data = contents.encode("utf-8")
        checksum = hashlib.new(name, data).hexdigest()
        return cls(name=name, checksum=checksum)


@dataclass
class Docs(dbtClassMixin):
    show: bool = True
    node_color: Optional[str] = None
