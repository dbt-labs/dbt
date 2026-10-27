from collections import ChainMap
from typing import Any, Dict, List, MutableMapping, Optional

from dbt.clients.jinja import MacroGenerator, MacroStack
from dbt.contracts.graph.nodes import Macro
from dbt.exceptions import PackageNotFoundForMacroError
from dbt.include.global_project import PROJECT_NAME as GLOBAL_PROJECT_NAME


class MacroNamespace(ChainMap):
    """A "virtual" namespace of macros. Rather than eagerly wrapping every macro
    in a MacroGenerator up front, this resolves names lazily: a MacroGenerator is
    only created when a macro is actually looked up (and therefore about to be
    called). Sub-namespaces (keyed by package name) are resolved the same way."""

    def __init__(
        self,
        ctx: Dict[str, Any],
        node,
        thread_ctx: MacroStack,
        search_dicts: List[MutableMapping[str, Any]],
    ) -> None:
        self.ctx = ctx
        self.node = node
        self.thread_ctx = thread_ctx
        super().__init__(*search_dicts)

    def __getitem__(self, key: str):
        value = super().__getitem__(key)
        if isinstance(value, Macro):
            return MacroGenerator(value, self.ctx, self.node, self.thread_ctx)
        elif isinstance(value, MutableMapping):
            return MacroNamespace(self.ctx, self.node, self.thread_ctx, [value])
        return value

    def get_from_package(self, package_name: Optional[str], name: str) -> Optional[MacroGenerator]:
        if package_name is None:
            return self.get(name)

        # A package name can collide with a macro name (e.g. a project named
        # after a macro). Resolve the package to its sub-namespace directly,
        # skipping any flat macro of the same name, rather than relying on
        # __getitem__ resolution order (which would return a MacroGenerator for
        # the colliding macro and blow up on `.get`).
        for mapping in self.maps:
            member = mapping.get(package_name)
            if isinstance(member, MutableMapping):
                sub_namespace = MacroNamespace(self.ctx, self.node, self.thread_ctx, [member])
                return sub_namespace.get(name)

        raise PackageNotFoundForMacroError(package_name)


# This class builds the MacroNamespace by assembling the ordered list of
# dictionaries the "virtual" MacroNamespace searches through.
# Call 'build_namespace' to return a MacroNamespace.
# This is used by ManifestContext (and subclasses)
class MacroNamespaceBuilder:
    def __init__(
        self,
        root_package: str,
        search_package: str,
        thread_ctx: MacroStack,
        internal_packages: List[str],
        node: Optional[Any] = None,
    ) -> None:
        self.root_package: str = root_package
        self.search_package: str = search_package
        self.thread_ctx: MacroStack = thread_ctx
        self.internal_packages: List[str] = internal_packages  # order significant
        self.node = node

    def build_namespace(
        self,
        macros_by_package: MutableMapping[str, MutableMapping[str, Macro]],
        ctx: Dict[str, Any],
    ) -> MacroNamespace:

        internals: ChainMap = ChainMap(
            *[
                macros_by_package[package_name]
                for package_name in self.internal_packages
                if package_name in macros_by_package
            ]
        )

        # The virtual namespace will attempt to resolve names into either macros
        # or sub-namespaces by checking the dictionaries in the following list
        # in order.
        search_dicts: List[MutableMapping] = [
            (
                macros_by_package[self.search_package]
                if self.search_package in macros_by_package
                and self.search_package not in self.internal_packages
                else {}
            ),
            macros_by_package[self.root_package] if self.root_package in macros_by_package else {},
            {k: v for k, v in macros_by_package.items() if k not in self.internal_packages},
            {
                GLOBAL_PROJECT_NAME: internals
            },  # Macros from internal packages are available within the 'dbt' namespace.
            internals,
        ]

        return MacroNamespace(ctx, self.node, self.thread_ctx, search_dicts)
