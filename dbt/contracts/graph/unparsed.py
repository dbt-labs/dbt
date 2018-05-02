from voluptuous import Schema, Required, All, Any, Length, Optional


from dbt.api import APIObject
from dbt.compat import basestring
from dbt.contracts.common import validate_with

from dbt.node_types import NodeType
from dbt.utils import deep_merge

unparsed_base_contract = Schema({
    # identifiers
    Required('name'): All(basestring, Length(min=1, max=127)),
    Required('package_name'): basestring,

    # filesystem
    Required('root_path'): basestring,
    Required('path'): basestring,
    Required('original_file_path'): basestring,
    Required('raw_sql'): basestring,
    Optional('index'): int,
})

unparsed_node_contract = unparsed_base_contract.extend({
    Required('resource_type'): Any(NodeType.Model,
                                   NodeType.Test,
                                   NodeType.Analysis,
                                   NodeType.Operation,
                                   NodeType.Seed)
})

unparsed_nodes_contract = Schema([unparsed_node_contract])


UNPARSED_BASE_CONTRACT = {
    'type': 'object',
    'additionalProperties': False,
    'properties': {
        'name': {
            'type': 'string',
            'description': (
                'Name of this node. For models, this is used as the '
                'identifier in the database.'),
            'minLength': 1,
            'maxLength': 127,
        },
        'package_name': {
            'type': 'string',
        },
        # filesystem
        'root_path': {
            'type': 'string',
            # TODO figure this out
            'description': '??? i think it is an absolute path to the file',
        },
        'path': {
            'type': 'string',
            'description': (
                'Relative path to the source file from the project root. '
                'Usually the same as original_file_path, but in some cases '
                'dbt will generate a path.'),
        },
        'original_file_path': {
            'type': 'string',
            'description': (
                'Relative path to the originating file from the project root.'
                ),
        },
        'raw_sql': {
            'type': 'string',
            'description': (
                'For nodes defined in SQL files, this is just the contents '
                'of that file. For schema tests, archives, etc. this is '
                'generated by dbt.'),
        },
        'index': {
            'type': 'integer',
        }
    },
    'required': ['name', 'package_name', 'root_path', 'path',
                 'original_file_path', 'raw_sql']
}

UNPARSED_NODE_CONTRACT = deep_merge(
    UNPARSED_BASE_CONTRACT,
    {
        'properties': {
            'resource_type': {
                'enum': [
                    NodeType.Model,
                    NodeType.Test,
                    NodeType.Analysis,
                    NodeType.Operation,
                    NodeType.Seed,
                ]
            }
        },
        'required': UNPARSED_BASE_CONTRACT['required'] + ['resource_type']
    }
)


class UnparsedNode(APIObject):
    SCHEMA = UNPARSED_NODE_CONTRACT
