from importlib import import_module

from pathlib import Path

from typing import Dict, Type, Any, Optional, List, Set

import threading

from dbt.adapters.base import AdapterPlugin
from dbt.adapters.contracts.connection import Credentials, AdapterRequiredConfig
from dbt.adapters.include.global_project import (
    PROJECT_NAME as GLOBAL_PROJECT_NAME,
    PACKAGE_PATH as GLOBAL_PROJECT_PATH,
)
from dbt.adapters import load_adapter
from dbt.adapters.protocol import RelationProtocol, AdapterConfig, AdapterProtocol
from dbt.common.events.functions import fire_event
from dbt.common.exceptions import DbtRuntimeError, DbtInternalError
from dbt.common.semver import VersionSpecifier
from dbt.events.types import AdapterRegistered
from dbt.mp_context import get_mp_context


class AdapterContainer:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.adapters: Dict[str, AdapterProtocol] = {}
        self.plugins: Dict[str, AdapterPlugin] = {}
        # map package names to their include paths
        self.packages: Dict[str, Path] = {
            GLOBAL_PROJECT_NAME: Path(GLOBAL_PROJECT_PATH),
        }

    def get_plugin_by_name(self, name: str) -> AdapterPlugin:
        with self.lock:
            if name in self.plugins:
                return self.plugins[name]
            names = ", ".join(self.plugins.keys())

        message = f"Invalid adapter type {name}! Must be one of {names}"
        raise DbtRuntimeError(message)

    def get_adapter_class_by_name(self, name: str) -> Type[AdapterProtocol]:
        plugin = self.get_plugin_by_name(name)
        return plugin.adapter

    def get_relation_class_by_name(self, name: str) -> Type[RelationProtocol]:
        adapter = self.get_adapter_class_by_name(name)
        return adapter.Relation

    def get_config_class_by_name(self, name: str) -> Type[AdapterConfig]:
        adapter = self.get_adapter_class_by_name(name)
        return adapter.AdapterSpecificConfigs

    def load_plugin(self, name: str) -> Type[Credentials]:
        # this doesn't need a lock: in the worst case we'll overwrite packages
        # and adapter_type entries with the same value, as they're all
        # singletons
        mod: Any = load_adapter.get_adapter_by_name(name)

        plugin: AdapterPlugin = mod.Plugin
        plugin_type = plugin.adapter.type()

        if plugin_type != name:
            raise DbtRuntimeError(
                f"Expected to find adapter with type named {name}, got "
                f"adapter with type {plugin_type}"
            )

        with self.lock:
            # things do hold the lock to iterate over it so we need it to add
            self.plugins[name] = plugin

        self.packages[plugin.project_name] = Path(plugin.include_path)

        for dep in plugin.dependencies:
            self.load_plugin(dep)

        return plugin.credentials

    def register_adapter(self, config: AdapterRequiredConfig) -> None:
        adapter_name = config.credentials.type
        adapter_type = self.get_adapter_class_by_name(adapter_name)
        adapter_version = import_module(f".{adapter_name}.__version__", "dbt.adapters").version
        adapter_version_specifier = VersionSpecifier.from_version_string(
            adapter_version
        ).to_version_string()
        fire_event(
            AdapterRegistered(adapter_name=adapter_name, adapter_version=adapter_version_specifier)
        )
        with self.lock:
            if adapter_name in self.adapters:
                # this shouldn't really happen...
                return

            adapter: AdapterProtocol = adapter_type(config, get_mp_context())  # type: ignore
            self.adapters[adapter_name] = adapter

    def lookup_adapter(self, adapter_name: str) -> AdapterProtocol:
        return self.adapters[adapter_name]

    def reset_adapters(self):
        """Clear the adapters. This is useful for tests, which change configs."""
        with self.lock:
            for adapter in self.adapters.values():
                adapter.cleanup_connections()
            self.adapters.clear()

    def cleanup_connections(self):
        """Only clean up the adapter connections list without resetting the
        actual adapters.
        """
        with self.lock:
            for adapter in self.adapters.values():
                adapter.cleanup_connections()

    def get_adapter_plugins(self, name: Optional[str]) -> List[AdapterPlugin]:
        """Iterate over the known adapter plugins. If a name is provided,
        iterate in dependency order over the named plugin and its dependencies.
        """
        if name is None:
            return list(self.plugins.values())

        plugins: List[AdapterPlugin] = []
        seen: Set[str] = set()
        plugin_names: List[str] = [name]
        while plugin_names:
            plugin_name = plugin_names[0]
            plugin_names = plugin_names[1:]
            try:
                plugin = self.plugins[plugin_name]
            except KeyError:
                raise DbtInternalError(f"No plugin found for {plugin_name}") from None
            plugins.append(plugin)
            seen.add(plugin_name)
            for dep in plugin.dependencies:
                if dep not in seen:
                    plugin_names.append(dep)
        return plugins

    def get_adapter_package_names(self, name: Optional[str]) -> List[str]:
        package_names: List[str] = [p.project_name for p in self.get_adapter_plugins(name)]
        package_names.append(GLOBAL_PROJECT_NAME)
        return package_names

    def get_include_paths(self, name: Optional[str]) -> List[Path]:
        paths = []
        for package_name in self.get_adapter_package_names(name):
            try:
                path = self.packages[package_name]
            except KeyError:
                raise DbtInternalError(f"No internal package listing found for {package_name}")
            paths.append(path)
        return paths

    def get_adapter_type_names(self, name: Optional[str]) -> List[str]:
        return [p.adapter.type() for p in self.get_adapter_plugins(name)]

    def get_adapter_constraint_support(self, name: Optional[str]) -> List[str]:
        return self.lookup_adapter(name).CONSTRAINT_SUPPORT  # type: ignore
