from contextlib import contextmanager

from pathlib import Path

from typing import Type, Optional, List
from dbt.adapters.contracts.connection import AdapterRequiredConfig, Credentials
from dbt.adapters.protocol import AdapterProtocol, AdapterConfig, RelationProtocol
from dbt.clients.adapter.container import AdapterContainer

_FACTORY: AdapterContainer = AdapterContainer()


def register_adapter(config: AdapterRequiredConfig) -> None:
    global _FACTORY
    _FACTORY.register_adapter(config)


def get_adapter(config: AdapterRequiredConfig):
    global _FACTORY
    return _FACTORY.lookup_adapter(config.credentials.type)


def get_adapter_by_type(adapter_type):
    global _FACTORY
    return _FACTORY.lookup_adapter(adapter_type)


def reset_adapters():
    """Clear the adapters. This is useful for tests, which change configs."""
    global _FACTORY
    _FACTORY.reset_adapters()


def cleanup_connections():
    """Only clean up the adapter connections list without resetting the actual
    adapters.
    """
    global _FACTORY
    _FACTORY.cleanup_connections()


def get_adapter_class_by_name(name: str) -> Type[AdapterProtocol]:
    global _FACTORY
    return _FACTORY.get_adapter_class_by_name(name)


def get_config_class_by_name(name: str) -> Type[AdapterConfig]:
    global _FACTORY
    return _FACTORY.get_config_class_by_name(name)


def get_relation_class_by_name(name: str) -> Type[RelationProtocol]:
    global _FACTORY
    return _FACTORY.get_relation_class_by_name(name)


def load_plugin(name: str) -> Type[Credentials]:
    global _FACTORY
    return _FACTORY.load_plugin(name)


def get_include_paths(name: Optional[str]) -> List[Path]:
    global _FACTORY
    return _FACTORY.get_include_paths(name)


def get_adapter_package_names(name: Optional[str]) -> List[str]:
    global _FACTORY
    return _FACTORY.get_adapter_package_names(name)


def get_adapter_type_names(name: Optional[str]) -> List[str]:
    global _FACTORY
    return _FACTORY.get_adapter_type_names(name)


def get_adapter_constraint_support(name: Optional[str]) -> List[str]:
    global _FACTORY
    return _FACTORY.get_adapter_constraint_support(name)


@contextmanager
def adapter_management():
    reset_adapters()
    try:
        yield
    finally:
        cleanup_connections()
