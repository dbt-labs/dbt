import json
import os

from dbt.adapters.factory import get_adapter
from dbt.clients.system import write_file
from dbt.compat import bigint
from dbt.include import GLOBAL_DBT_MODULES_PATH
from dbt.node_runners import OperationRunner
from dbt.node_types import NodeType
from dbt.runner import RunManager
import dbt.ui.printer

from dbt.task.base_task import BaseTask


CATALOG_FILENAME = 'catalog.json'
OPERATION_NAME = 'get_catalog_data'


def get_stripped_prefix(source, prefix):
    """Go through source, extracting every key/value pair where the key starts
    with the given prefix.
    """
    cut = len(prefix)
    return {
        k[cut:]: v for k, v in source.items()
        if k.startswith(prefix)
    }


def unflatten(columns):
    """Given a list of column dictionaries following this layout:

        [{
            'column_comment': None,
            'column_index': Decimal('1'),
            'column_name': 'id',
            'column_type': 'integer',
            'table_comment': None,
            'table_name': 'test_table',
            'table_schema': 'test_schema',
            'table_type': 'BASE TABLE'
        }]

    unflatten will convert them into a dict with this nested structure:

        {
            'test_schema': {
                'test_table': {
                    'metadata': {
                        'comment': None,
                        'name': 'test_table',
                        'type': 'BASE TABLE',
                        'schema': 'test_schema',
                    },
                    'columns': [
                        {
                            'type': 'integer',
                            'comment': None,
                            'index': bigint(1),
                            'name': 'id'
                        }
                    ]
                }
            }
        }

    Required keys in each column: table_schema, table_name, column_index

    Keys prefixed with 'column_' end up in per-column data and keys prefixed
    with 'table_' end up in table metadata. Keys without either prefix are
    ignored.
    """
    structured = {}
    for entry in columns:
        schema_name = entry['table_schema']
        table_name = entry['table_name']

        if schema_name not in structured:
            structured[schema_name] = {}
        schema = structured[schema_name]

        if table_name not in schema:
            metadata = get_stripped_prefix(entry, 'table_')
            schema[table_name] = {'metadata': metadata, 'columns': []}
        table = schema[table_name]

        column = get_stripped_prefix(entry, 'column_')
        # the index should really never be that big so it's ok to end up
        # serializing this to JSON (2^53 is the max safe value there)
        column['index'] = bigint(column['index'])
        table['columns'].append(column)
    return structured


# derive from BaseTask as I don't really want any result interpretation.
class GenerateTask(BaseTask):
    def get_all_projects(self):
        root_project = self.project.cfg
        all_projects = {root_project.get('name'): root_project}
        # we only need to load the global deps. We haven't compiled, so our
        # project['module-path'] does not exist.
        dependency_projects = dbt.utils.dependencies_for_path(
            self.project, GLOBAL_DBT_MODULES_PATH
        )

        for project in dependency_projects:
            name = project.cfg.get('name', 'unknown')
            all_projects[name] = project.cfg

        if dbt.flags.STRICT_MODE:
            dbt.contracts.project.ProjectList(**all_projects)

        return all_projects

    def _get_flat_graph(self):
        # TODO: I'd like to do this better. We can't use
        # utils.dependency_projects because it assumes you have compiled your
        # project (I think?) - it assumes that you have an existing and
        # populated project['modules-path'], but 'catalog generate' shouldn't
        # require that. It might be better to suppress the exception in
        # dependency_projects if that's reasonable, or make it a flag.
        root_project = self.project.cfg
        all_projects = self.get_all_projects()

        manifest = dbt.loader.GraphLoader.load_all(root_project, all_projects)
        return manifest.to_flat_graph()

    def _get_adapter(self):
        profile = self.project.run_environment()
        return get_adapter(profile)

    def run(self):
        flat_graph = self._get_flat_graph()
        operation = dbt.utils.get_operation_macro(flat_graph, OPERATION_NAME)
        adapter = self._get_adapter()

        # TODO: this really feels like it should be along the lines of
        # agate_table = adapter.run_operation(flat_graph, operation)
        runner = OperationRunner(self.project, adapter, operation, 0, 0)
        run_result = runner.safe_run(flat_graph)
        agate_table = run_result.returned

        results = [
            dict(zip(agate_table.column_names, row))
            for row in agate_table
        ]
        results = unflatten(results)

        path = os.path.join(self.project['target-path'], CATALOG_FILENAME)
        write_file(path, json.dumps(results))

        dbt.ui.printer.print_timestamped_line(
            'Catalog written to {}'.format(os.path.abspath(path))
        )

        return results
