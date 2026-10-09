import os

import pytest

from dbt.tests.util import get_manifest, run_dbt, write_file

common_dbt_project_yml = """
name: common_pkg
version: '1.0'
config-version: 2
"""

common_seed_csv = """id,name
1,Alice
2,Bob
"""

common_seed_schema_yml = """
version: 2
seeds:
  - name: shared_seed
    description: "The common package's version of the seed."
"""

root_seed_csv = """id,name
1,Charlie
2,Dana
3,Erin
"""

root_seed_schema_yml = """
version: 2
seeds:
  - name: shared_seed
    description: "This project's own override of the seed."
"""


class TestDisabledDependencySeedSchemaPatch:
    """
    Regression test for https://github.com/dbt-labs/dbt-core/issues/15562

    A common pattern for projects built on top of a shared package: disable
    the package's seed via `+enabled: false` in the root project's
    dbt_project.yml, and define a same-named seed directly in the root
    project instead. Both the package and the root project describe the
    seed (with the same name) in their own schema.yml.

    Parsing used to fail with "dbt found two schema.yml entries for the
    same resource named shared_seed" even though only one schema.yml
    entry actually describes an *enabled* resource. The disabled package's
    own schema.yml patch was resolving, via a cross-package name lookup,
    to the root project's unrelated (and already-patched) node instead of
    being recognized as describing the package's own disabled seed.
    """

    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "shared_seed.csv": root_seed_csv,
            "schema.yml": root_seed_schema_yml,
        }

    @pytest.fixture(scope="class")
    def packages(self):
        return {"packages": [{"local": "common_pkg"}]}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"seeds": {"common_pkg": {"+enabled": False}}}

    @pytest.fixture(scope="class", autouse=True)
    def prepare_dependencies(self, project):
        pkg_root = os.path.join(project.project_root, "common_pkg")
        os.makedirs(os.path.join(pkg_root, "seeds"), exist_ok=True)
        write_file(common_dbt_project_yml, pkg_root, "dbt_project.yml")
        write_file(common_seed_csv, pkg_root, "seeds", "shared_seed.csv")
        write_file(common_seed_schema_yml, pkg_root, "seeds", "schema.yml")

    def test_parses_without_duplicate_patch_error(self, project):
        run_dbt(["deps"])
        # Before the fix, this raised DuplicatePatchPathError: "dbt found
        # two schema.yml entries for the same resource named shared_seed."
        run_dbt(["parse"])

        manifest = get_manifest(project.project_root)

        # The root project's own (enabled) seed is the one that gets
        # patched with the root project's schema.yml description.
        root_seed_id = "seed.test.shared_seed"
        assert root_seed_id in manifest.nodes
        assert manifest.nodes[root_seed_id].patch_path is not None
        assert (
            manifest.nodes[root_seed_id].description
            == "This project's own override of the seed."
        )

        # The common package's seed is disabled, as configured, and is
        # patched with its own package's schema.yml description rather
        # than being conflated with the root project's seed.
        disabled_seeds = [
            node
            for nodes in manifest.disabled.values()
            for node in nodes
            if node.name == "shared_seed"
        ]
        assert len(disabled_seeds) == 1
        assert disabled_seeds[0].unique_id.startswith("seed.common_pkg.")
        assert disabled_seeds[0].patch_path is not None
        assert (
            disabled_seeds[0].description
            == "The common package's version of the seed."
        )
