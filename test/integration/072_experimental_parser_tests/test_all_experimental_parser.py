from dbt.contracts.graph.manifest import Manifest
import os
from test.integration.base import DBTIntegrationTest, use_profile


def get_manifest():
    path = './target/partial_parse.msgpack'
    if os.path.exists(path):
        with open(path, 'rb') as fp:
            manifest_mp = fp.read()
        manifest: Manifest = Manifest.from_msgpack(manifest_mp)
        return manifest
    else:
        return None


class TestBasicExperimentalParser(DBTIntegrationTest):
    @property
    def schema(self):
        return "072_basic"

    @property
    def models(self):
        return "basic"

    @use_profile('postgres')
    def test_postgres_experimental_parser_basic(self):
        results = self.run_dbt(['--use-experimental-parser', 'parse'])
        manifest = get_manifest()
        node = manifest.nodes['model.test.model_a']
        self.assertEqual(node.refs, [['model_a']])
        self.assertEqual(node.sources, [['my_src', 'my_tbl']])
        self.assertEqual(node.config._extra, {'x': True})
        self.assertEqual(node.config.tags, ['hello', 'world'])

class TestExperimentalParserRefMacroDetection(DBTIntegrationTest):
    @property
    def schema(self):
        return "072_ref_macro_detection"

    @property
    def models(self):
        return "ref_macro_detection"

    @use_profile('postgres')
    def test_postgres_experimental_parser_ref_macro_detection(self):
        results = self.run_dbt(['--use-experimental-parser', 'parse'])
        manifest = get_manifest()
        node = manifest.nodes['model.test.model_a']
        # TODO this assert really won't fail.
        self.assertEqual(node.refs, [['model_b']])
