import functools
from typing import NoReturn

from dbt.events.functions import warn_or_error
from dbt.events.helpers import env_secrets, scrub_secrets
from dbt.events.types import JinjaLogWarning

from dbt.exceptions import (
    DbtRuntimeError,
    MissingConfigError,
    MissingMaterializationError,
    MissingRelationError,
    AmbiguousAliasError,
    AmbiguousCatalogMatchError,
    CacheInconsistencyError,
    DataclassNotDictError,
    CompilationError,
    DbtDatabaseError,
    DependencyNotFoundError,
    DependencyError,
    DuplicatePatchPathError,
    DuplicateResourceNameError,
    PropertyYMLError,
    NotImplementedError,
    RelationWrongTypeError,
    ContractError,
    ColumnTypeMissingError,
    FailFastError,
)


def warn(msg, node=None):
    warn_or_error(JinjaLogWarning(msg=msg), node=node)
    return ""


def missing_config(model, name) -> NoReturn:
    raise MissingConfigError(unique_id=model.unique_id, name=name)


def missing_materialization(model, adapter_type) -> NoReturn:
    raise MissingMaterializationError(
        materialization=model.config.materialized, adapter_type=adapter_type
    )


def missing_relation(relation, model=None) -> NoReturn:
    raise MissingRelationError(relation, model)


def raise_ambiguous_alias(node_1, node_2, duped_name=None) -> NoReturn:
    raise AmbiguousAliasError(node_1, node_2, duped_name)


def raise_ambiguous_catalog_match(unique_id, match_1, match_2) -> NoReturn:
    raise AmbiguousCatalogMatchError(unique_id, match_1, match_2)


def raise_cache_inconsistent(message) -> NoReturn:
    raise CacheInconsistencyError(message)


def raise_dataclass_not_dict(obj) -> NoReturn:
    raise DataclassNotDictError(obj)


def raise_compiler_error(msg, node=None) -> NoReturn:
    raise CompilationError(msg, node)


def raise_contract_error(yaml_columns, sql_columns) -> NoReturn:
    raise ContractError(yaml_columns, sql_columns)


def raise_database_error(msg, node=None) -> NoReturn:
    raise DbtDatabaseError(msg, node)


def raise_dep_not_found(node, node_description, required_pkg) -> NoReturn:
    raise DependencyNotFoundError(node, node_description, required_pkg)


def raise_dependency_error(msg) -> NoReturn:
    raise DependencyError(scrub_secrets(msg, env_secrets()))


def raise_duplicate_patch_name(patch_1, existing_patch_path) -> NoReturn:
    raise DuplicatePatchPathError(patch_1, existing_patch_path)


def raise_duplicate_resource_name(node_1, node_2) -> NoReturn:
    raise DuplicateResourceNameError(node_1, node_2)


def raise_invalid_property_yml_version(path, issue) -> NoReturn:
    raise PropertyYMLError(path, issue)


def raise_not_implemented(msg) -> NoReturn:
    raise NotImplementedError(msg)


def relation_wrong_type(relation, expected_type, model=None) -> NoReturn:
    raise RelationWrongTypeError(relation, expected_type, model)


def column_type_missing(column_names) -> NoReturn:
    raise ColumnTypeMissingError(column_names)


def raise_fail_fast_error(msg, node=None) -> NoReturn:
    raise FailFastError(msg, node=node)


# Update this when a new function should be added to the
# dbt context's `exceptions` key!
CONTEXT_EXPORTS = {
    fn.__name__: fn
    for fn in [
        warn,
        missing_config,
        missing_materialization,
        missing_relation,
        raise_ambiguous_alias,
        raise_ambiguous_catalog_match,
        raise_cache_inconsistent,
        raise_dataclass_not_dict,
        raise_compiler_error,
        raise_database_error,
        raise_dep_not_found,
        raise_dependency_error,
        raise_duplicate_patch_name,
        raise_duplicate_resource_name,
        raise_invalid_property_yml_version,
        raise_not_implemented,
        relation_wrong_type,
        raise_contract_error,
        column_type_missing,
        raise_fail_fast_error,
    ]
}


# wraps context based exceptions in node info
def wrapper(model):
    def wrap(func):
        @functools.wraps(func)
        def inner(*args, **kwargs):
            try:
                return func(*args, **kwargs)
            except DbtRuntimeError as exc:
                exc.add_node(model)
                raise exc

        return inner

    return wrap


def wrapped_exports(model):
    wrap = wrapper(model)
    return {name: wrap(export) for name, export in CONTEXT_EXPORTS.items()}
