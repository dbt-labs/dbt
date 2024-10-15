import re
from datetime import date
from typing import Any, Callable, Dict, Optional, Tuple, Union

from dbt.adapters.contracts.connection import HasCredentials
from dbt.clients.jinja import get_rendered
from dbt.constants import DEPENDENCIES_FILE_NAME, SECRET_PLACEHOLDER
from dbt.context.base import BaseContext
from dbt.context.secret import SecretContext
from dbt.context.target import TargetContext
from dbt.exceptions import DbtProjectError
from dbt_common.clients.jinja import catch_jinja
from dbt_common.constants import SECRET_ENV_PREFIX
from dbt_common.context import get_invocation_context
from dbt_common.exceptions import CompilationError, RecursionError
from dbt_common.utils import deep_map_render

Keypath = Tuple[Union[str, int], ...]


class BaseRenderer:
    def __init__(self, context: Dict[str, Any]) -> None:
        self.context = context

    @property
    def name(self):
        return "Rendering"

    def should_render_keypath(self, keypath: Keypath) -> bool:
        return True

    def render_entry(self, value: Any, keypath: Keypath) -> Any:
        if not self.should_render_keypath(keypath):
            return value

        return self.render_value(value, keypath)

    def render_value(self, value: Any, keypath: Optional[Keypath] = None) -> Any:
        # keypath is ignored (and someone who knows should explain why here)
        if not isinstance(value, str):
            return value if not isinstance(value, date) else value.isoformat()

        try:
            with catch_jinja():
                return get_rendered(value, self.context, native=True)
        except CompilationError as exc:
            msg = f"Could not render {value}: {exc.msg}"
            raise CompilationError(msg) from exc

    def render_data(self, data: Dict[str, Any]) -> Dict[str, Any]:
        try:
            return deep_map_render(self.render_entry, data)
        except RecursionError:
            raise DbtProjectError(
                f"Cycle detected: {self.name} input has a reference to itself", project=data
            )


def _list_if_none(value):
    if value is None:
        value = []
    return value


def _dict_if_none(value):
    if value is None:
        value = {}
    return value


def _list_if_none_or_string(value):
    value = _list_if_none(value)
    if isinstance(value, str):
        return [value]
    return value


class ProjectPostprocessor(Dict[Keypath, Callable[[Any], Any]]):
    def __init__(self) -> None:
        super().__init__()

        self[("on-run-start",)] = _list_if_none_or_string
        self[("on-run-end",)] = _list_if_none_or_string

        for k in ("models", "seeds", "snapshots"):
            self[(k,)] = _dict_if_none
            self[(k, "vars")] = _dict_if_none
            self[(k, "pre-hook")] = _list_if_none_or_string
            self[(k, "post-hook")] = _list_if_none_or_string
        self[("seeds", "column_types")] = _dict_if_none

    def postprocess(self, value: Any, key: Keypath) -> Any:
        if key in self:
            handler = self[key]
            return handler(value)

        return value


class DbtProjectYamlRenderer(BaseRenderer):
    _KEYPATH_HANDLERS = ProjectPostprocessor()

    def __init__(
        self, profile: Optional[HasCredentials] = None, cli_vars: Optional[Dict[str, Any]] = None
    ) -> None:
        # Generate contexts here because we want to save the context
        # object in order to retrieve the env_vars. This is almost always
        # a TargetContext, but in the debug task we want a project
        # even when we don't have a profile.
        if cli_vars is None:
            cli_vars = {}
        if profile:
            self.ctx_obj = TargetContext(profile.to_target_dict(), cli_vars)
        else:
            self.ctx_obj = BaseContext(cli_vars)  # type:ignore
        context = self.ctx_obj.to_dict()
        super().__init__(context)

    @property
    def name(self):
        "Project config"

    # Uses SecretRenderer
    def get_package_renderer(self) -> BaseRenderer:
        return PackageRenderer(self.ctx_obj.cli_vars)

    def render_project(
        self,
        project: Dict[str, Any],
        project_root: str,
    ) -> Dict[str, Any]:
        """Render the project and insert the project root after rendering."""
        rendered_project = self.render_data(project)
        rendered_project["project-root"] = project_root
        return rendered_project

    def render_packages(self, packages: Dict[str, Any], packages_specified_path: str):
        """Render the given packages dict"""
        packages = packages or {}  # Sometimes this is none in tests
        package_renderer = self.get_package_renderer()
        if packages_specified_path == DEPENDENCIES_FILE_NAME:
            # We don't want to render the "packages" dictionary that came from dependencies.yml
            return packages
        else:
            return package_renderer.render_data(packages)

    def render_selectors(self, selectors: Dict[str, Any]):
        return self.render_data(selectors)

    def render_entry(self, value: Any, keypath: Keypath) -> Any:
        result = super().render_entry(value, keypath)
        return self._KEYPATH_HANDLERS.postprocess(result, keypath)

    def should_render_keypath(self, keypath: Keypath) -> bool:
        if not keypath:
            return True

        first = keypath[0]
        # run hooks are not rendered
        if first in {"on-run-start", "on-run-end", "query-comment"}:
            return False

        # don't render vars blocks until runtime
        if first == "vars":
            return False

        if first in {"seeds", "models", "snapshots", "tests", "data_tests"}:
            keypath_parts = {(k.lstrip("+ ") if isinstance(k, str) else k) for k in keypath}
            # model-level hooks
            late_rendered_hooks = {"pre-hook", "post-hook", "pre_hook", "post_hook"}
            if keypath_parts.intersection(late_rendered_hooks):
                return False

        return True


class SecretRenderer(BaseRenderer):
    def __init__(self, cli_vars: Dict[str, Any] = {}) -> None:
        # Generate contexts here because we want to save the context
        # object in order to retrieve the env_vars.
        self.ctx_obj = SecretContext(cli_vars)
        context = self.ctx_obj.to_dict()
        super().__init__(context)

    @property
    def name(self):
        return "Secret"

    def render_value(self, value: Any, keypath: Optional[Keypath] = None) -> Any:
        # First, standard Jinja rendering, with special handling for 'secret' environment variables
        # "{{ env_var('DBT_SECRET_ENV_VAR') }}" -> "$$$DBT_SECRET_START$$$DBT_SECRET_ENV_{VARIABLE_NAME}$$$DBT_SECRET_END$$$"
        # This prevents Jinja manipulation of secrets via macros/filters that might leak partial/modified values in logs

        try:
            rendered = super().render_value(value, keypath)
        except Exception as ex:
            if keypath and "password" in keypath:
                # Passwords sometimes contain jinja-esque characters, but we
                # don't want to render them if they aren't valid jinja.
                rendered = value
            else:
                raise ex

        # Now, detect instances of the placeholder value ($$$DBT_SECRET_START...DBT_SECRET_END$$$)
        # and replace them with the actual secret value
        if SECRET_ENV_PREFIX in str(rendered):
            search_group = f"({SECRET_ENV_PREFIX}(.*))"
            pattern = SECRET_PLACEHOLDER.format(search_group).replace("$", r"\$")
            m = re.search(
                pattern,
                rendered,
            )
            if m:
                found = m.group(1)
                value = get_invocation_context().env[found]
                replace_this = SECRET_PLACEHOLDER.format(found)
                return rendered.replace(replace_this, value)
        else:
            return rendered


class ProfileRenderer(SecretRenderer):
    @property
    def name(self):
        return "Profile"


class PackageRenderer(SecretRenderer):
    @property
    def name(self):
        return "Packages config"


class CatalogRenderer(SecretRenderer):
    @property
    def name(self):
        return "Catalog config"
