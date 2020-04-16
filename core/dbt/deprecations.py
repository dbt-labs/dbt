from typing import Optional, Set, List, Dict, ClassVar

import dbt.links
import dbt.exceptions
import dbt.flags
from dbt.ui import printer


class DBTDeprecation:
    _name: ClassVar[Optional[str]] = None
    _description: ClassVar[Optional[str]] = None

    @property
    def name(self) -> str:
        if self._name is not None:
            return self._name
        raise NotImplementedError(
            'name not implemented for {}'.format(self)
        )

    @property
    def description(self) -> str:
        if self._description is not None:
            return self._description
        raise NotImplementedError(
            'description not implemented for {}'.format(self)
        )

    def show(self, *args, **kwargs) -> None:
        if self.name not in active_deprecations:
            desc = self.description.format(**kwargs)
            msg = printer.line_wrap_message(
                desc, prefix='* Deprecation Warning: '
            )
            dbt.exceptions.warn_or_error(msg)
            active_deprecations.add(self.name)


class MaterializationReturnDeprecation(DBTDeprecation):
    _name = 'materialization-return'

    _description = '''\
    The materialization ("{materialization}") did not explicitly return a list
    of relations to add to the cache. By default the target relation will be
    added, but this behavior will be removed in a future version of dbt.



    For more information, see:

    https://docs.getdbt.com/v0.15/docs/creating-new-materializations#section-6-returning-relations
    '''


class NotADictionaryDeprecation(DBTDeprecation):
    _name = 'not-a-dictionary'

    _description = '''\
    The object ("{obj}") was used as a dictionary. In a future version of dbt
    this capability will be removed from objects of this type.
    '''


class ColumnQuotingDeprecation(DBTDeprecation):
    _name = 'column-quoting-unset'

    _description = '''\
    The quote_columns parameter was not set for seeds, so the default value of
    False was chosen. The default will change to True in a future release.



    For more information, see:

    https://docs.getdbt.com/v0.15/docs/seeds#section-specify-column-quoting
    '''


class ModelsKeyNonModelDeprecation(DBTDeprecation):
    _name = 'models-key-mismatch'

    _description = '''\
    "{node.name}" is a {node.resource_type} node, but it is specified in
    the {patch.yaml_key} section of {patch.original_file_path}.



    To fix this warning, place the `{node.name}` specification under
    the {expected_key} key instead.

    This warning will become an error in a future release.
    '''


class DbtProjectYamlDeprecation(DBTDeprecation):
    _name = 'dbt-project-yaml-v1'
    _description = '''\
    The existing dbt_project.yml format has been deprecated. dbt_project.yml
    has been upgraded to config version 2. A future version of dbt will remove
    support for the existing ("version 1") format.

    Documentation for dbt_project.yml version 2 can be found here:

        DOCS LINK GOES HERE
    '''


_adapter_renamed_description = """\
The adapter function `adapter.{old_name}` is deprecated and will be removed in
a future release of dbt. Please use `adapter.{new_name}` instead.

Documentation for {new_name} can be found here:

    https://docs.getdbt.com/docs/adapter
"""


def renamed_method(old_name: str, new_name: str):

    class AdapterDeprecationWarning(DBTDeprecation):
        _name = 'adapter:{}'.format(old_name)
        _description = _adapter_renamed_description.format(old_name=old_name,
                                                           new_name=new_name)

    dep = AdapterDeprecationWarning()
    deprecations_list.append(dep)
    deprecations[dep.name] = dep


def warn(name, *args, **kwargs):
    if name not in deprecations:
        # this should (hopefully) never happen
        raise RuntimeError(
            "Error showing deprecation warning: {}".format(name)
        )

    deprecations[name].show(*args, **kwargs)


# these are globally available
# since modules are only imported once, active_deprecations is a singleton

active_deprecations: Set[str] = set()

deprecations_list: List[DBTDeprecation] = [
    MaterializationReturnDeprecation(),
    NotADictionaryDeprecation(),
    ColumnQuotingDeprecation(),
    ModelsKeyNonModelDeprecation(),
    DbtProjectYamlDeprecation(),
]

deprecations: Dict[str, DBTDeprecation] = {
    d.name: d for d in deprecations_list
}


def reset_deprecations():
    active_deprecations.clear()
