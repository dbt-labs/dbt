from mock import MagicMock
import unittest

import os

import dbt.flags
import dbt.compilation
from collections import OrderedDict


class CompilerTest(unittest.TestCase):

    def assertEqualIgnoreWhitespace(self, a, b):
        self.assertEqual(
            "".join(a.split()),
            "".join(b.split()))

    def setUp(self):
        dbt.flags.STRICT_MODE = True

        self.maxDiff = None

        self.root_project_config = {
            'name': 'root_project',
            'version': '0.1',
            'profile': 'test',
            'project-root': os.path.abspath('.'),
        }

        self.snowplow_project_config = {
            'name': 'snowplow',
            'version': '0.1',
            'project-root': os.path.abspath('./dbt_modules/snowplow'),
        }

        self.model_config = {
            'enabled': True,
            'materialized': 'view',
            'post-hook': [],
            'pre-hook': [],
            'vars': {},
        }

    def test__prepend_ctes__already_has_cte(self):
        ephemeral_config = self.model_config.copy()
        ephemeral_config['materialized'] = 'ephemeral'

        compiled_models = {
            'model.root.view': {
                'name': 'view',
                'resource_type': 'model',
                'unique_id': 'model.root.view',
                'fqn': ['root_project', 'view'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [
                    'model.root.ephemeral'
                ],
                'config': self.model_config,
                'tags': set(),
                'path': 'view.sql',
                'raw_sql': 'select * from {{ref("ephemeral")}}',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict([
                    ('model.root.ephemeral', None)
                ]),
                'injected_sql': '',
                'compiled_sql': ('with cte as (select * from something_else) '
                                 'select * from __dbt__CTE__ephemeral')
            },
            'model.root.ephemeral': {
                'name': 'ephemeral',
                'resource_type': 'model',
                'unique_id': 'model.root.ephemeral',
                'fqn': ['root_project', 'ephemeral'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': ephemeral_config,
                'tags': set(),
                'path': 'ephemeral.sql',
                'raw_sql': 'select * from source_table',
                'compiled': True,
                'compiled_sql': 'select * from source_table',
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict(),
                'injected_sql': ''
            }
        }

        result, all_models = dbt.compilation.prepend_ctes(
            compiled_models['model.root.view'],
            compiled_models)

        self.assertEqual(result, all_models.get('model.root.view'))
        self.assertEqual(result.get('extra_ctes_injected'), True)
        self.assertEqualIgnoreWhitespace(
            result.get('injected_sql'),
            ('with __dbt__CTE__ephemeral as ('
             'select * from source_table'
             '), cte as (select * from something_else) '
             'select * from __dbt__CTE__ephemeral'))

        self.assertEqual(
            all_models.get('model.root.ephemeral').get('extra_ctes_injected'),
            True)

    def test__prepend_ctes__no_ctes(self):
        compiled_models = {
            'model.root.view': {
                'name': 'view',
                'resource_type': 'model',
                'unique_id': 'model.root.view',
                'fqn': ['root_project', 'view'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': self.model_config,
                'tags': set(),
                'path': 'view.sql',
                'raw_sql': ('with cte as (select * from something_else) '
                            'select * from source_table'),
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict(),
                'injected_sql': '',
                'compiled_sql': ('with cte as (select * from something_else) '
                                 'select * from source_table')
            },
            'model.root.view_no_cte': {
                'name': 'view_no_cte',
                'resource_type': 'model',
                'unique_id': 'model.root.view_no_cte',
                'fqn': ['root_project', 'view_no_cte'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': self.model_config,
                'tags': set(),
                'path': 'view.sql',
                'raw_sql': 'select * from source_table',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict(),
                'injected_sql': '',
                'compiled_sql': ('select * from source_table')
            }
        }

        result, all_models = dbt.compilation.prepend_ctes(
            compiled_models.get('model.root.view'),
            compiled_models)

        self.assertEqual(result, all_models.get('model.root.view'))
        self.assertEqual(result.get('extra_ctes_injected'), True)
        self.assertEqualIgnoreWhitespace(
            result.get('injected_sql'),
            compiled_models.get('model.root.view').get('compiled_sql'))

        result, all_models = dbt.compilation.prepend_ctes(
            compiled_models.get('model.root.view_no_cte'),
            compiled_models)

        self.assertEqual(result, all_models.get('model.root.view_no_cte'))
        self.assertEqual(result.get('extra_ctes_injected'), True)
        self.assertEqualIgnoreWhitespace(
            result.get('injected_sql'),
            compiled_models.get('model.root.view_no_cte').get('compiled_sql'))

    def test__prepend_ctes(self):
        ephemeral_config = self.model_config.copy()
        ephemeral_config['materialized'] = 'ephemeral'

        compiled_models = {
            'model.root.view': {
                'name': 'view',
                'resource_type': 'model',
                'unique_id': 'model.root.view',
                'fqn': ['root_project', 'view'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [
                    'model.root.ephemeral'
                ],
                'config': self.model_config,
                'tags': set(),
                'path': 'view.sql',
                'raw_sql': 'select * from {{ref("ephemeral")}}',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict([
                    ('model.root.ephemeral', None)
                ]),
                'injected_sql': '',
                'compiled_sql': 'select * from __dbt__CTE__ephemeral'
            },
            'model.root.ephemeral': {
                'name': 'ephemeral',
                'resource_type': 'model',
                'unique_id': 'model.root.ephemeral',
                'fqn': ['root_project', 'ephemeral'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': ephemeral_config,
                'tags': set(),
                'path': 'ephemeral.sql',
                'raw_sql': 'select * from source_table',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict(),
                'injected_sql': '',
                'compiled_sql': 'select * from source_table'
            }
        }

        result, all_models = dbt.compilation.prepend_ctes(
            compiled_models['model.root.view'],
            compiled_models)

        self.assertEqual(result, all_models.get('model.root.view'))
        self.assertEqual(result.get('extra_ctes_injected'), True)
        self.assertEqualIgnoreWhitespace(
            result.get('injected_sql'),
            ('with __dbt__CTE__ephemeral as ('
             'select * from source_table'
             ') '
             'select * from __dbt__CTE__ephemeral'))

        self.assertEqual(
            all_models.get('model.root.ephemeral').get('extra_ctes_injected'),
            True)


    def test__prepend_ctes__multiple_levels(self):
        ephemeral_config = self.model_config.copy()
        ephemeral_config['materialized'] = 'ephemeral'

        compiled_models = {
            'model.root.view': {
                'name': 'view',
                'resource_type': 'model',
                'unique_id': 'model.root.view',
                'fqn': ['root_project', 'view'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [
                    'model.root.ephemeral'
                ],
                'config': self.model_config,
                'tags': set(),
                'path': 'view.sql',
                'raw_sql': 'select * from {{ref("ephemeral")}}',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict([
                    ('model.root.ephemeral', None)
                ]),
                'injected_sql': '',
                'compiled_sql': 'select * from __dbt__CTE__ephemeral'
            },
            'model.root.ephemeral': {
                'name': 'ephemeral',
                'resource_type': 'model',
                'unique_id': 'model.root.ephemeral',
                'fqn': ['root_project', 'ephemeral'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': ephemeral_config,
                'tags': set(),
                'path': 'ephemeral.sql',
                'raw_sql': 'select * from {{ref("ephemeral_level_two")}}',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict([
                    ('model.root.ephemeral_level_two', None)
                ]),
                'injected_sql': '',
                'compiled_sql': 'select * from __dbt__CTE__ephemeral_level_two'
            },
            'model.root.ephemeral_level_two': {
                'name': 'ephemeral_level_two',
                'resource_type': 'model',
                'unique_id': 'model.root.ephemeral_level_two',
                'fqn': ['root_project', 'ephemeral_level_two'],
                'empty': False,
                'package_name': 'root',
                'root_path': '/usr/src/app',
                'depends_on': [],
                'config': ephemeral_config,
                'tags': set(),
                'path': 'ephemeral_level_two.sql',
                'raw_sql': 'select * from source_table',
                'compiled': True,
                'extra_ctes_injected': False,
                'extra_cte_sql': OrderedDict(),
                'injected_sql': '',
                'compiled_sql': 'select * from source_table'
            }

        }

        result, all_models = dbt.compilation.prepend_ctes(
            compiled_models['model.root.view'],
            compiled_models)

        self.assertEqual(result, all_models.get('model.root.view'))
        self.assertEqual(result.get('extra_ctes_injected'), True)
        self.assertEqualIgnoreWhitespace(
            result.get('injected_sql'),
            ('with __dbt__CTE__ephemeral_level_two as ('
             'select * from source_table'
             '), __dbt__CTE__ephemeral as ('
             'select * from __dbt__CTE__ephemeral_level_two'
             ') '
             'select * from __dbt__CTE__ephemeral'))

        self.assertEqual(
            all_models.get('model.root.ephemeral').get('extra_ctes_injected'),
            True)
        self.assertEqual(
            all_models.get('model.root.ephemeral_level_two').get('extra_ctes_injected'),
            True)
