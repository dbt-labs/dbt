from collections.abc import Hashable
from dataclasses import dataclass, field
from typing import Optional, TypeVar, Any, Type, Dict, Iterator, Tuple, Set

from dbt.adapters.relation_configs import (
    RelationConfigChangeCollection,
    RelationResults,
)
from dbt.contracts.graph.nodes import SourceDefinition, ManifestNode, ResultNode, ParsedNode
from dbt.contracts.relation import (
    RelationType,
    ComponentName,
    HasQuoting,
    FakeAPIObject,
    Policy,
    Path,
)
from dbt.exceptions import (
    ApproximateMatchError,
    DbtInternalError,
    DbtRuntimeError,
    MultipleDatabasesNotAllowedError,
)
from dbt.node_types import NodeType
from dbt.utils import filter_null_values, deep_merge, classproperty

import dbt.exceptions


Self = TypeVar("Self", bound="BaseRelation")


@dataclass(frozen=True, eq=False, repr=False)
class BaseRelation(FakeAPIObject, Hashable):
    path: Path
    type: Optional[RelationType] = None
    quote_character: str = '"'
    # Python 3.11 requires that these use default_factory instead of simple default
    # ValueError: mutable default <class 'dbt.contracts.relation.Policy'> for field include_policy is not allowed: use default_factory
    include_policy: Policy = field(default_factory=lambda: Policy())
    quote_policy: Policy = field(default_factory=lambda: Policy())
    dbt_created: bool = False

    def _is_exactish_match(self, field: ComponentName, value: str) -> bool:
        if self.dbt_created and self.quote_policy.get_part(field) is False:
            return self.path.get_lowered_part(field) == value.lower()
        else:
            return self.path.get_part(field) == value

    @classmethod
    def _get_field_named(cls, field_name):
        for f, _ in cls._get_fields():
            if f.name == field_name:
                return f
        # this should be unreachable
        raise ValueError(f"BaseRelation has no {field_name} field!")

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return False
        return self.to_dict(omit_none=True) == other.to_dict(omit_none=True)

    @classmethod
    def get_default_quote_policy(cls) -> Policy:
        return cls._get_field_named("quote_policy").default_factory()

    @classmethod
    def get_default_include_policy(cls) -> Policy:
        return cls._get_field_named("include_policy").default_factory()

    def get(self, key, default=None):
        """Override `.get` to return a metadata object so we don't break
        dbt_utils.
        """
        if key == "metadata":
            return {"type": self.__class__.__name__}
        return super().get(key, default)

    def matches(
        self,
        database: Optional[str] = None,
        schema: Optional[str] = None,
        identifier: Optional[str] = None,
    ) -> bool:
        search = filter_null_values(
            {
                ComponentName.Database: database,
                ComponentName.Schema: schema,
                ComponentName.Identifier: identifier,
            }
        )

        if not search:
            # nothing was passed in
            raise dbt.exceptions.DbtRuntimeError(
                "Tried to match relation, but no search path was passed!"
            )

        exact_match = True
        approximate_match = True

        for k, v in search.items():
            if not self._is_exactish_match(k, v):
                exact_match = False
            if str(self.path.get_lowered_part(k)).strip(self.quote_character) != v.lower().strip(
                self.quote_character
            ):
                approximate_match = False  # type: ignore[union-attr]

        if approximate_match and not exact_match:
            target = self.create(database=database, schema=schema, identifier=identifier)
            raise ApproximateMatchError(target, self)

        return exact_match

    def replace_path(self, **kwargs):
        return self.replace(path=self.path.replace(**kwargs))

    def quote(
        self: Self,
        database: Optional[bool] = None,
        schema: Optional[bool] = None,
        identifier: Optional[bool] = None,
    ) -> Self:
        policy = filter_null_values(
            {
                ComponentName.Database: database,
                ComponentName.Schema: schema,
                ComponentName.Identifier: identifier,
            }
        )

        new_quote_policy = self.quote_policy.replace_dict(policy)
        return self.replace(quote_policy=new_quote_policy)

    def include(
        self: Self,
        database: Optional[bool] = None,
        schema: Optional[bool] = None,
        identifier: Optional[bool] = None,
    ) -> Self:
        policy = filter_null_values(
            {
                ComponentName.Database: database,
                ComponentName.Schema: schema,
                ComponentName.Identifier: identifier,
            }
        )

        new_include_policy = self.include_policy.replace_dict(policy)
        return self.replace(include_policy=new_include_policy)

    def information_schema(self, view_name=None) -> "InformationSchema":
        # some of our data comes from jinja, where things can be `Undefined`.
        if not isinstance(view_name, str):
            view_name = None

        # Kick the user-supplied schema out of the information schema relation
        # Instead address this as <database>.information_schema by default
        info_schema = InformationSchema.from_relation(self, view_name)
        return info_schema.incorporate(path={"schema": None})

    def information_schema_only(self) -> "InformationSchema":
        return self.information_schema()

    def without_identifier(self) -> "BaseRelation":
        """Return a form of this relation that only has the database and schema
        set to included. To get the appropriately-quoted form the schema out of
        the result (for use as part of a query), use `.render()`. To get the
        raw database or schema name, use `.database` or `.schema`.

        The hash of the returned object is the result of render().
        """
        return self.include(identifier=False).replace_path(identifier=None)

    def _render_iterator(self) -> Iterator[Tuple[Optional[ComponentName], Optional[str]]]:

        for key in ComponentName:
            path_part: Optional[str] = None
            if self.include_policy.get_part(key):
                path_part = self.path.get_part(key)
                if path_part is not None and self.quote_policy.get_part(key):
                    path_part = self.quoted(path_part)
            yield key, path_part

    def render(self) -> str:
        # if there is nothing set, this will return the empty string.
        return ".".join(part for _, part in self._render_iterator() if part is not None)

    def quoted(self, identifier):
        return "{quote_char}{identifier}{quote_char}".format(
            quote_char=self.quote_character,
            identifier=identifier,
        )

    @classmethod
    def create_from_source(cls: Type[Self], source: SourceDefinition, **kwargs: Any) -> Self:
        source_quoting = source.quoting.to_dict(omit_none=True)
        source_quoting.pop("column", None)
        quote_policy = deep_merge(
            cls.get_default_quote_policy().to_dict(omit_none=True),
            source_quoting,
            kwargs.get("quote_policy", {}),
        )

        return cls.create(
            database=source.database,
            schema=source.schema,
            identifier=source.identifier,
            quote_policy=quote_policy,
            **kwargs,
        )

    @staticmethod
    def add_ephemeral_prefix(name: str):
        return f"__dbt__cte__{name}"

    @classmethod
    def create_ephemeral_from_node(
        cls: Type[Self],
        config: HasQuoting,
        node: ManifestNode,
    ) -> Self:
        # Note that ephemeral models are based on the name.
        identifier = cls.add_ephemeral_prefix(node.name)
        return cls.create(
            type=cls.CTE,
            identifier=identifier,
        ).quote(identifier=False)

    @classmethod
    def create_from_node(
        cls: Type[Self],
        config: HasQuoting,
        node,
        quote_policy: Optional[Dict[str, bool]] = None,
        **kwargs: Any,
    ) -> Self:
        if quote_policy is None:
            quote_policy = {}

        quote_policy = dbt.utils.merge(config.quoting, quote_policy)

        return cls.create(
            database=node.database,
            schema=node.schema,
            identifier=node.alias,
            quote_policy=quote_policy,
            **kwargs,
        )

    @classmethod
    def create_from(
        cls: Type[Self],
        config: HasQuoting,
        node: ResultNode,
        **kwargs: Any,
    ) -> Self:
        if node.resource_type == NodeType.Source:
            if not isinstance(node, SourceDefinition):
                raise DbtInternalError(
                    "type mismatch, expected SourceDefinition but got {}".format(type(node))
                )
            return cls.create_from_source(node, **kwargs)
        else:
            # Can't use ManifestNode here because of parameterized generics
            if not isinstance(node, (ParsedNode)):
                raise DbtInternalError(
                    f"type mismatch, expected ManifestNode but got {type(node)}"
                )
            return cls.create_from_node(config, node, **kwargs)

    @classmethod
    def create(
        cls: Type[Self],
        database: Optional[str] = None,
        schema: Optional[str] = None,
        identifier: Optional[str] = None,
        type: Optional[RelationType] = None,
        **kwargs,
    ) -> Self:
        kwargs.update(
            {
                "path": {
                    "database": database,
                    "schema": schema,
                    "identifier": identifier,
                },
                "type": type,
            }
        )
        return cls.from_dict(kwargs)

    def get_relation_config_change_collection(
        self,
        relation_results: RelationResults,
        runtime_config,  # RuntimeConfigObject
    ) -> Optional[RelationConfigChangeCollection]:
        """
        Determine the relation config changes between an existing deployment and the new deployment.

        This could be overridden in the event that there is a adapter-specific materialization; however,
        it's recommended that the relation type-specific methods are overridden in all other scenarios.
        In the former scenario, the overridden method should still call to `super()` to catch all commonly
        supported materializations.

        Args:
            relation_results: the description of the existing deployment according to the database
            runtime_config: the description of the new deployment according to the user's config

        Returns: a set of changes to be made on the relation; this would be None if there are no changes
        """
        relation_type = runtime_config.model.config.get("materialized", RelationType.default())
        if relation_type == RelationType.MaterializedView:
            return self.get_materialized_view_config_change_collection(
                relation_results, runtime_config
            )
        raise DbtRuntimeError(
            f"Config changes have not been configured for relation type {relation_type}."
        )

    def get_materialized_view_config_change_collection(
        self,
        relation_results: RelationResults,
        runtime_config,  # RuntimeConfigObject
    ) -> Optional[RelationConfigChangeCollection]:
        raise NotImplementedError(
            "Materialized view config changes have not been configured for this adapter."
        )

    def __repr__(self) -> str:
        return "<{} {}>".format(self.__class__.__name__, self.render())

    def __hash__(self) -> int:
        return hash(self.render())

    def __str__(self) -> str:
        return self.render()

    @property
    def database(self) -> Optional[str]:
        return self.path.database

    @property
    def schema(self) -> Optional[str]:
        return self.path.schema

    @property
    def identifier(self) -> Optional[str]:
        return self.path.identifier

    @property
    def table(self) -> Optional[str]:
        return self.path.identifier

    # Here for compatibility with old Relation interface
    @property
    def name(self) -> Optional[str]:
        return self.identifier

    @property
    def is_table(self) -> bool:
        return self.type == RelationType.Table

    @property
    def is_cte(self) -> bool:
        return self.type == RelationType.CTE

    @property
    def is_view(self) -> bool:
        return self.type == RelationType.View

    @property
    def is_materialized_view(self) -> bool:
        return self.type == RelationType.MaterializedView

    @classproperty
    def Table(cls) -> str:
        return str(RelationType.Table)

    @classproperty
    def CTE(cls) -> str:
        return str(RelationType.CTE)

    @classproperty
    def View(cls) -> str:
        return str(RelationType.View)

    @classproperty
    def External(cls) -> str:
        return str(RelationType.External)

    @classproperty
    def MaterializedView(cls) -> str:
        return str(RelationType.MaterializedView)

    @classproperty
    def get_relation_type(cls) -> Type[RelationType]:
        return RelationType


Info = TypeVar("Info", bound="InformationSchema")


@dataclass(frozen=True, eq=False, repr=False)
class InformationSchema(BaseRelation):
    information_schema_view: Optional[str] = None

    def __post_init__(self):
        if not isinstance(self.information_schema_view, (type(None), str)):
            raise dbt.exceptions.CompilationError(
                "Got an invalid name: {}".format(self.information_schema_view)
            )

    @classmethod
    def get_path(cls, relation: BaseRelation, information_schema_view: Optional[str]) -> Path:
        return Path(
            database=relation.database,
            schema=relation.schema,
            identifier="INFORMATION_SCHEMA",
        )

    @classmethod
    def get_include_policy(
        cls,
        relation,
        information_schema_view: Optional[str],
    ) -> Policy:
        return relation.include_policy.replace(
            database=relation.database is not None,
            schema=False,
            identifier=True,
        )

    @classmethod
    def get_quote_policy(
        cls,
        relation,
        information_schema_view: Optional[str],
    ) -> Policy:
        return relation.quote_policy.replace(
            identifier=False,
        )

    @classmethod
    def from_relation(
        cls: Type[Info],
        relation: BaseRelation,
        information_schema_view: Optional[str],
    ) -> Info:
        include_policy = cls.get_include_policy(relation, information_schema_view)
        quote_policy = cls.get_quote_policy(relation, information_schema_view)
        path = cls.get_path(relation, information_schema_view)
        return cls(
            type=RelationType.View,
            path=path,
            include_policy=include_policy,
            quote_policy=quote_policy,
            information_schema_view=information_schema_view,
        )

    def _render_iterator(self):
        for k, v in super()._render_iterator():
            yield k, v
        yield None, self.information_schema_view


class SchemaSearchMap(Dict[InformationSchema, Set[Optional[str]]]):
    """A utility class to keep track of what information_schema tables to
    search for what schemas. The schema values are all lowercased to avoid
    duplication.
    """

    def add(self, relation: BaseRelation):
        key = relation.information_schema_only()
        if key not in self:
            self[key] = set()
        schema: Optional[str] = None
        if relation.schema is not None:
            schema = relation.schema.lower()
        self[key].add(schema)

    def search(self) -> Iterator[Tuple[InformationSchema, Optional[str]]]:
        for information_schema_name, schemas in self.items():
            for schema in schemas:
                yield information_schema_name, schema

    def flatten(self, allow_multiple_databases: bool = False):
        new = self.__class__()

        # make sure we don't have multiple databases if allow_multiple_databases is set to False
        if not allow_multiple_databases:
            seen = {r.database.lower() for r in self if r.database}
            if len(seen) > 1:
                raise MultipleDatabasesNotAllowedError(seen)

        for information_schema_name, schema in self.search():
            path = {"database": information_schema_name.database, "schema": schema}
            new.add(
                information_schema_name.incorporate(
                    path=path,
                    quote_policy={"database": False},
                    include_policy={"database": False},
                )
            )

        return new
