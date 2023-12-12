import traceback
from importlib import import_module
from types import ModuleType


from dbt.adapters.events.types import AdapterImportError, PluginLoadError
from dbt.common.events.functions import fire_event
from dbt.common.exceptions import DbtRuntimeError


def get_adapter_by_name(name: str) -> ModuleType:
    try:
        # mypy doesn't think modules have any attributes.
        mod: ModuleType = import_module("." + name, "dbt.adapters")
        return mod
    except ModuleNotFoundError as exc:
        # if we failed to import the target module in particular, inform
        # the user about it via a runtime error
        if exc.name == "dbt.adapters." + name:
            fire_event(AdapterImportError(exc=str(exc)))
            raise DbtRuntimeError(f"Could not find adapter type {name}!")
        # otherwise, the error had to have come from some underlying
        # library. Log the stack trace.

        fire_event(PluginLoadError(exc_info=traceback.format_exc()))
        raise
