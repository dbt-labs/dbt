import os
from copy import deepcopy
from typing import MutableMapping, Dict, List, Callable
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.files import (
    AnySourceFile,
    ParseFileType,
    parse_file_type_to_parser,
    SchemaSourceFile,
)
from dbt.events.functions import fire_event
from dbt.events.base_types import EventLevel
from dbt.events.types import (
    PartialParsingEnabled,
    PartialParsingFile,
)
from dbt.constants import DEFAULT_ENV_PLACEHOLDER
from dbt.node_types import NodeType


mssat_files = (
    ParseFileType.Model,
    ParseFileType.Seed,
    ParseFileType.Snapshot,
    ParseFileType.Analysis,
    ParseFileType.SingularTest,
)

mg_files = (
    ParseFileType.Macro,
    ParseFileType.GenericTest,
)


key_to_prefix = {
    "models": "model",
    "seeds": "seed",
    "snapshots": "snapshot",
    "analyses": "analysis",
}


parse_file_type_to_key = {
    ParseFileType.Model: "models",
    ParseFileType.Seed: "seeds",
    ParseFileType.Snapshot: "snapshots",
    ParseFileType.Analysis: "analyses",
}


# These macro names have special treatment in the ManifestLoader and
# partial parsing. If they have changed we will skip partial parsing
special_override_macros = [
    "ref",
    "source",
    "config",
    "generate_schema_name",
    "generate_database_name",
    "generate_alias_name",
]


# Partial parsing. Create a diff of files from saved manifest and current
# files and produce a project_parser_file dictionary to drive parsing of
# only the necessary changes.
# Will produce a 'skip_parsing' method, and a project_parser_file dictionary
# All file objects from the new manifest are deepcopied, because we need
# to preserve an unchanged file object in case we need to drop back to a
# a full parse (such as for certain macro changes)
class PartialParsing:
    def __init__(
        self, saved_manifest: Manifest, new_files: MutableMapping[str, AnySourceFile]
    ) -> None:
        self.saved_manifest = saved_manifest
        self.new_files = new_files
        self.project_parser_files: Dict = {}
        self.saved_files = self.saved_manifest.files
        self.project_parser_files = {}
        self.macro_child_map: Dict[str, List[str]] = {}
        (
            self.env_vars_changed_source_files,
            self.env_vars_changed_schema_files,
        ) = self.build_env_vars_to_files()
        self.build_file_diff()
        self.processing_file = None
        self.deleted_special_override_macro = False
        self.disabled_by_file_id = self.saved_manifest.build_disabled_by_file_id()

    def skip_parsing(self):
        return (
            not self.file_diff["deleted"]
            and not self.file_diff["added"]
            and not self.file_diff["changed"]
            and not self.file_diff["changed_schema_files"]
            and not self.file_diff["deleted_schema_files"]
        )

    # Compare the previously saved manifest files and the just-loaded manifest
    # files to see if anything changed
    def build_file_diff(self):
        saved_file_ids = set(self.saved_files.keys())
        new_file_ids = set(self.new_files.keys())
        deleted_all_files = saved_file_ids.difference(new_file_ids)
        added = new_file_ids.difference(saved_file_ids)
        common = saved_file_ids.intersection(new_file_ids)
        changed_or_deleted_macro_file = False

        # separate out deleted schema files
        deleted_schema_files = []
        deleted = []
        for file_id in deleted_all_files:
            if self.saved_files[file_id].parse_file_type == ParseFileType.Schema:
                deleted_schema_files.append(file_id)
            else:
                if self.saved_files[file_id].parse_file_type in mg_files:
                    changed_or_deleted_macro_file = True
                deleted.append(file_id)

        changed = []
        changed_schema_files = []
        unchanged = []
        for file_id in common:
            if self.saved_files[file_id].checksum == self.new_files[file_id].checksum:
                unchanged.append(file_id)
            else:
                # separate out changed schema files
                if self.saved_files[file_id].parse_file_type == ParseFileType.Schema:
                    sf = self.saved_files[file_id]
                    if type(sf).__name__ != "SchemaSourceFile":
                        raise Exception(f"Serialization failure for {file_id}")
                    changed_schema_files.append(file_id)
                else:
                    if self.saved_files[file_id].parse_file_type in mg_files:
                        changed_or_deleted_macro_file = True
                    changed.append(file_id)

        # handle changed env_vars for non-schema-files
        for file_id in self.env_vars_changed_source_files:
            if file_id in deleted or file_id in changed:
                continue
            changed.append(file_id)

        # handle changed env_vars for schema files
        for file_id in self.env_vars_changed_schema_files.keys():
            if file_id in deleted_schema_files or file_id in changed_schema_files:
                continue
            changed_schema_files.append(file_id)

        file_diff = {
            "deleted": deleted,
            "deleted_schema_files": deleted_schema_files,
            "added": added,
            "changed": changed,
            "changed_schema_files": changed_schema_files,
            "unchanged": unchanged,
        }
        if changed_or_deleted_macro_file:
            self.macro_child_map = self.saved_manifest.build_macro_child_map()
        deleted = len(deleted) + len(deleted_schema_files)
        changed = len(changed) + len(changed_schema_files)
        event = PartialParsingEnabled(deleted=deleted, added=len(added), changed=changed)
        if os.environ.get("DBT_PP_TEST"):
            fire_event(event, level=EventLevel.INFO)
        else:
            fire_event(event)
        self.file_diff = file_diff

    # generate the list of files that need parsing
    # uses self.manifest.files generated by 'read_files'
    def get_parsing_files(self):
        if self.skip_parsing():
            return {}
        # Need to add new files first, because changes in schema files
        # might refer to them
        for file_id in self.file_diff["added"]:
            self.processing_file = file_id
            self.add_to_saved(file_id)
        # Need to process schema files next, because the dictionaries
        # need to be in place for handling SQL file changes
        for file_id in self.file_diff["changed_schema_files"]:
            self.processing_file = file_id
            self.change_schema_file(file_id)
        for file_id in self.file_diff["deleted_schema_files"]:
            self.processing_file = file_id
            self.delete_schema_file(file_id)
        for file_id in self.file_diff["deleted"]:
            self.processing_file = file_id
            self.delete_from_saved(file_id)
        for file_id in self.file_diff["changed"]:
            self.processing_file = file_id
            self.update_in_saved(file_id)
        return self.project_parser_files

    # Add the file to the project parser dictionaries to schedule parsing
    def add_to_pp_files(self, source_file):
        file_id = source_file.file_id
        parser_name = parse_file_type_to_parser[source_file.parse_file_type]
        project_name = source_file.project_name
        if not parser_name or not project_name:
            raise Exception(
                f"Did not find parse_file_type or project_name "
                f"in SourceFile for {source_file.file_id}"
            )
        if project_name not in self.project_parser_files:
            self.project_parser_files[project_name] = {}
        if parser_name not in self.project_parser_files[project_name]:
            self.project_parser_files[project_name][parser_name] = []
        if (
            file_id not in self.project_parser_files[project_name][parser_name]
            and file_id not in self.file_diff["deleted"]
        ):
            self.project_parser_files[project_name][parser_name].append(file_id)

    def already_scheduled_for_parsing(self, source_file):
        file_id = source_file.file_id
        project_name = source_file.project_name
        if project_name not in self.project_parser_files:
            return False
        parser_name = parse_file_type_to_parser[source_file.parse_file_type]
        if parser_name not in self.project_parser_files[project_name]:
            return False
        if file_id not in self.project_parser_files[project_name][parser_name]:
            return False
        return True

    # Add new files, including schema files
    def add_to_saved(self, file_id):
        # add file object to saved manifest.files
        source_file = deepcopy(self.new_files[file_id])
        if source_file.parse_file_type == ParseFileType.Schema:
            self.handle_added_schema_file(source_file)
        self.saved_files[file_id] = source_file
        # update pp_files to parse
        self.add_to_pp_files(source_file)
        fire_event(PartialParsingFile(operation="added", file_id=file_id))

    def handle_added_schema_file(self, source_file):
        source_file.pp_dict = source_file.dict_from_yaml.copy()
        if "sources" in source_file.pp_dict:
            for source in source_file.pp_dict["sources"]:
                # We need to remove the original source, so it can
                # be properly patched
                if "overrides" in source:
                    self.remove_source_override_target(source)

    def delete_disabled(self, unique_id, file_id):
        # This node/metric/exposure is disabled. Find it and remove it from disabled dictionary.
        for dis_index, dis_node in enumerate(self.saved_manifest.disabled[unique_id]):
            if dis_node.file_id == file_id:
                node = dis_node
                index = dis_index
                break
        # Remove node from disabled
        del self.saved_manifest.disabled[unique_id][index]
        # if all nodes were removed for the unique id, delete the unique_id
        # from the disabled dict
        if not self.saved_manifest.disabled[unique_id]:
            self.saved_manifest.disabled.pop(unique_id)

        return node

    # Deletes for all non-schema files
    def delete_from_saved(self, file_id):
        # Look at all things touched by file, remove those
        # nodes, and update pp_files to parse unless the
        # file creating those nodes has also been deleted
        saved_source_file = self.saved_files[file_id]

        # SQL file: models, seeds, snapshots, analyses, tests: SQL files, except
        # macros/tests
        if saved_source_file.parse_file_type in mssat_files:
            self.remove_mssat_file(saved_source_file)
            self.saved_manifest.files.pop(file_id)

        # macros
        if saved_source_file.parse_file_type in mg_files:
            self.delete_macro_file(saved_source_file, follow_references=True)

        # docs
        if saved_source_file.parse_file_type == ParseFileType.Documentation:
            self.delete_doc_node(saved_source_file)

        fire_event(PartialParsingFile(operation="deleted", file_id=file_id))

    # Updates for non-schema files
    def update_in_saved(self, file_id):
        new_source_file = deepcopy(self.new_files[file_id])
        old_source_file = self.saved_files[file_id]

        if new_source_file.parse_file_type in mssat_files:
            self.update_mssat_in_saved(new_source_file, old_source_file)
        elif new_source_file.parse_file_type in mg_files:
            self.update_macro_in_saved(new_source_file, old_source_file)
        elif new_source_file.parse_file_type == ParseFileType.Documentation:
            self.update_doc_in_saved(new_source_file, old_source_file)
        else:
            raise Exception(f"Invalid parse_file_type in source_file {file_id}")
        fire_event(PartialParsingFile(operation="updated", file_id=file_id))

    # Models, seeds, snapshots: patches and tests
    # analyses: patches, no tests
    # tests: not touched by schema files (no patches, no tests)
    # Updated schema files should have been processed already.
    def update_mssat_in_saved(self, new_source_file, old_source_file):

        if self.already_scheduled_for_parsing(old_source_file):
            return

        # These files only have one node except for snapshots and
        # model files with yaml frontmatter
        unique_ids = []
        if old_source_file.nodes:
            unique_ids = old_source_file.nodes

        # replace source_file in saved and add to parsing list
        file_id = new_source_file.file_id
        self.saved_files[file_id] = deepcopy(new_source_file)
        self.add_to_pp_files(new_source_file)
        for unique_id in unique_ids:
            self.remove_node_in_saved(new_source_file, unique_id)

    def remove_node_in_saved(self, source_file, unique_id):
        if unique_id in self.saved_manifest.nodes:
            # delete node in saved
            node = self.saved_manifest.nodes.pop(unique_id)
        elif (
            source_file.file_id in self.disabled_by_file_id
            and unique_id in self.saved_manifest.disabled
        ):
            # This node is disabled. Find the node and remove it from disabled dictionary.
            node = self.delete_disabled(unique_id, source_file.file_id)
        else:
            # Has already been deleted by another action
            return

        # look at patch_path in model node to see if we need
        # to reapply a patch from a schema_file (skipping nodes with
        # a patch_path that indicates yaml frontmatter patch).
        if node.patch_path and node.patch_path != node.file_id:
            file_id = node.patch_path
            # it might be changed...  then what?
            if file_id not in self.file_diff["deleted"] and file_id in self.saved_files:
                # Schema files should already be updated if this comes from a node,
                # but this code is also called when updating groups and exposures.
                # This might save the old schema file element, so when the schema file
                # is processed, it should overwrite it by passing True to "merge_patch"
                schema_file = self.saved_files[file_id]
                dict_key = parse_file_type_to_key[source_file.parse_file_type]
                # look for a matching list dictionary
                elem_patch = None
                if dict_key in schema_file.dict_from_yaml:
                    for elem in schema_file.dict_from_yaml[dict_key]:
                        if elem["name"] == node.name:
                            elem_patch = elem
                            break
                if elem_patch:
                    self.delete_schema_mssa_links(schema_file, dict_key, elem_patch)
                    self.merge_patch(schema_file, dict_key, elem_patch)
                    if unique_id in schema_file.node_patches:
                        schema_file.node_patches.remove(unique_id)
            if unique_id in self.saved_manifest.disabled:
                # We have a patch_path in disabled nodes with a patch so
                # that we can connect the patch to the node
                for node in self.saved_manifest.disabled[unique_id]:
                    node.patch_path = None

    def update_macro_in_saved(self, new_source_file, old_source_file):
        if self.already_scheduled_for_parsing(old_source_file):
            return
        self.handle_macro_file_links(old_source_file, follow_references=True)
        file_id = new_source_file.file_id
        self.saved_files[file_id] = deepcopy(new_source_file)
        self.add_to_pp_files(new_source_file)

    def update_doc_in_saved(self, new_source_file, old_source_file):
        if self.already_scheduled_for_parsing(old_source_file):
            return
        self.delete_doc_node(old_source_file)
        self.saved_files[new_source_file.file_id] = deepcopy(new_source_file)
        self.add_to_pp_files(new_source_file)

    def remove_mssat_file(self, source_file):
        # nodes [unique_ids] -- SQL files
        # There should always be a node for a SQL file
        if not source_file.nodes:
            return
        # There is generally only 1 node for SQL files, except for macros and snapshots
        for unique_id in source_file.nodes:
            self.remove_node_in_saved(source_file, unique_id)
            self.schedule_referencing_nodes_for_parsing(unique_id)

    # We need to re-parse nodes that reference another removed node
    def schedule_referencing_nodes_for_parsing(self, unique_id):
        # Look at "children", i.e. nodes that reference this node
        if unique_id in self.saved_manifest.child_map:
            self.schedule_nodes_for_parsing(self.saved_manifest.child_map[unique_id])

    def schedule_nodes_for_parsing(self, unique_ids):
        for unique_id in unique_ids:
            if unique_id in self.saved_manifest.nodes:
                node = self.saved_manifest.nodes[unique_id]
                if node.resource_type == NodeType.Test and node.test_node_type == "generic":
                    # test nodes are handled separately. Must be removed from schema file
                    continue
                file_id = node.file_id
                if file_id in self.saved_files and file_id not in self.file_diff["deleted"]:
                    source_file = self.saved_files[file_id]
                    self.remove_mssat_file(source_file)
                    # content of non-schema files is only in new files
                    self.saved_files[file_id] = deepcopy(self.new_files[file_id])
                    self.add_to_pp_files(self.saved_files[file_id])
            elif unique_id in self.saved_manifest.sources:
                source = self.saved_manifest.sources[unique_id]
                self._schedule_for_parsing(
                    "sources", source, source.source_name, self.delete_schema_source
                )
            elif unique_id in self.saved_manifest.exposures:
                exposure = self.saved_manifest.exposures[unique_id]
                self._schedule_for_parsing(
                    "exposures", exposure, exposure.name, self.delete_schema_exposure
                )
            elif unique_id in self.saved_manifest.metrics:
                metric = self.saved_manifest.metrics[unique_id]
                self._schedule_for_parsing(
                    "metrics", metric, metric.name, self.delete_schema_metric
                )
            elif unique_id in self.saved_manifest.semantic_models:
                semantic_model = self.saved_manifest.semantic_models[unique_id]
                self._schedule_for_parsing(
                    "semantic_models",
                    semantic_model,
                    semantic_model.name,
                    self.delete_schema_semantic_model,
                )
            elif unique_id in self.saved_manifest.saved_queries:
                saved_query = self.saved_manifest.saved_queries[unique_id]
                self._schedule_for_parsing(
                    "saved_queries", saved_query, saved_query.name, self.delete_schema_saved_query
                )
            elif unique_id in self.saved_manifest.macros:
                macro = self.saved_manifest.macros[unique_id]
                file_id = macro.file_id
                if file_id in self.saved_files and file_id not in self.file_diff["deleted"]:
                    source_file = self.saved_files[file_id]
                    self.delete_macro_file(source_file)
                    self.saved_files[file_id] = deepcopy(self.new_files[file_id])
                    self.add_to_pp_files(self.saved_files[file_id])

    def _schedule_for_parsing(self, dict_key: str, element, name, delete: Callable) -> None:
        file_id = element.file_id
        if file_id in self.saved_files and file_id not in self.file_diff["deleted"]:
            schema_file = self.saved_files[file_id]
            elements = []
            assert isinstance(schema_file, SchemaSourceFile)
            if dict_key in schema_file.dict_from_yaml:
                elements = schema_file.dict_from_yaml[dict_key]
            schema_element = self.get_schema_element(elements, name)
            if schema_element:
                delete(schema_file, schema_element)
                self.merge_patch(schema_file, dict_key, schema_element)

    def delete_macro_file(self, source_file, follow_references=False):
        self.check_for_special_deleted_macros(source_file)
        self.handle_macro_file_links(source_file, follow_references)
        file_id = source_file.file_id
        # It's not clear when this file_id would not exist in saved_files
        if file_id in self.saved_files:
            self.saved_files.pop(file_id)

    def check_for_special_deleted_macros(self, source_file):
        for unique_id in source_file.macros:
            if unique_id in self.saved_manifest.macros:
                package_name = unique_id.split(".")[1]
                if package_name == "dbt":
                    continue
                macro = self.saved_manifest.macros[unique_id]
                if macro.name in special_override_macros:
                    self.deleted_special_override_macro = True

    def recursively_gather_macro_references(self, macro_unique_id, referencing_nodes):
        for unique_id in self.macro_child_map[macro_unique_id]:
            if unique_id in referencing_nodes:
                continue
            referencing_nodes.append(unique_id)
            if unique_id.startswith("macro."):
                self.recursively_gather_macro_references(unique_id, referencing_nodes)

    def handle_macro_file_links(self, source_file, follow_references=False):
        # remove the macros in the 'macros' dictionary
        macros = source_file.macros.copy()
        for unique_id in macros:
            if unique_id not in self.saved_manifest.macros:
                # This happens when a macro has already been removed
                if unique_id in source_file.macros:
                    source_file.macros.remove(unique_id)
                continue

            base_macro = self.saved_manifest.macros.pop(unique_id)

            # Recursively check children of this macro
            # The macro_child_map might not exist if a macro is removed by
            # schedule_nodes_for parsing. We only want to follow
            # references if the macro file itself has been updated or
            # deleted, not if we're just updating referenced nodes.
            if self.macro_child_map and follow_references:
                referencing_nodes = []
                self.recursively_gather_macro_references(unique_id, referencing_nodes)
                self.schedule_macro_nodes_for_parsing(referencing_nodes)

            if base_macro.patch_path:
                file_id = base_macro.patch_path
                if file_id in self.saved_files:
                    schema_file = self.saved_files[file_id]
                    macro_patches = []
                    if "macros" in schema_file.dict_from_yaml:
                        macro_patches = schema_file.dict_from_yaml["macros"]
                    macro_patch = self.get_schema_element(macro_patches, base_macro.name)
                    self.delete_schema_macro_patch(schema_file, macro_patch)
                    self.merge_patch(schema_file, "macros", macro_patch)
            # The macro may have already been removed by handling macro children
            if unique_id in source_file.macros:
                source_file.macros.remove(unique_id)

    # similar to schedule_nodes_for_parsing but doesn't do sources and exposures
    # and handles schema tests
    def schedule_macro_nodes_for_parsing(self, unique_ids):
        for unique_id in unique_ids:
            if unique_id in self.saved_manifest.nodes:
                node = self.saved_manifest.nodes[unique_id]
                # Both generic tests from yaml files and singular tests have NodeType.Test
                # so check for generic test.
                if node.resource_type == NodeType.Test and node.test_node_type == "generic":
                    schema_file_id = node.file_id
                    schema_file = self.saved_manifest.files[schema_file_id]
                    (key, name) = schema_file.get_key_and_name_for_test(node.unique_id)
                    if key and name:
                        patch_list = []
                        if key in schema_file.dict_from_yaml:
                            patch_list = schema_file.dict_from_yaml[key]
                        patch = self.get_schema_element(patch_list, name)
                        if patch:
                            if key in ["models", "seeds", "snapshots"]:
                                self.delete_schema_mssa_links(schema_file, key, patch)
                                self.merge_patch(schema_file, key, patch)
                                if unique_id in schema_file.node_patches:
                                    schema_file.node_patches.remove(unique_id)
                            elif key == "sources":
                                # re-schedule source
                                if "overrides" in patch:
                                    # This is a source patch; need to re-parse orig source
                                    self.remove_source_override_target(patch)
                                self.delete_schema_source(schema_file, patch)
                                self.merge_patch(schema_file, "sources", patch)
                else:
                    file_id = node.file_id
                    if file_id in self.saved_files and file_id not in self.file_diff["deleted"]:
                        source_file = self.saved_files[file_id]
                        self.remove_mssat_file(source_file)
                        # content of non-schema files is only in new files
                        self.saved_files[file_id] = deepcopy(self.new_files[file_id])
                        self.add_to_pp_files(self.saved_files[file_id])
            elif unique_id in self.saved_manifest.macros:
                macro = self.saved_manifest.macros[unique_id]
                file_id = macro.file_id
                if file_id in self.saved_files and file_id not in self.file_diff["deleted"]:
                    source_file = self.saved_files[file_id]
                    self.delete_macro_file(source_file)
                    self.saved_files[file_id] = deepcopy(self.new_files[file_id])
                    self.add_to_pp_files(self.saved_files[file_id])

    def delete_doc_node(self, source_file):
        # remove the nodes in the 'docs' dictionary
        docs = source_file.docs.copy()
        for unique_id in docs:
            self.saved_manifest.docs.pop(unique_id)
            source_file.docs.remove(unique_id)
        # The unique_id of objects that contain a doc call are stored in the
        # doc source_file.nodes
        self.schedule_nodes_for_parsing(source_file.nodes)
        source_file.nodes = []
        # Remove the file object
        self.saved_manifest.files.pop(source_file.file_id)

    # Schema files -----------------------
    # Changed schema files
    def change_schema_file(self, file_id):
        saved_schema_file = self.saved_files[file_id]
        new_schema_file = deepcopy(self.new_files[file_id])
        saved_yaml_dict = saved_schema_file.dict_from_yaml
        new_yaml_dict = new_schema_file.dict_from_yaml
        saved_schema_file.pp_dict = {}
        self.handle_schema_file_changes(saved_schema_file, saved_yaml_dict, new_yaml_dict)

        # copy from new schema_file to saved_schema_file to preserve references
        # that weren't removed
        saved_schema_file.contents = new_schema_file.contents
        saved_schema_file.checksum = new_schema_file.checksum
        saved_schema_file.dfy = new_schema_file.dfy
        # schedule parsing
        self.add_to_pp_files(saved_schema_file)
        # schema_file pp_dict should have been generated already
        fire_event(PartialParsingFile(operation="updated", file_id=file_id))

    # Delete schema files -- a variation on change_schema_file
    def delete_schema_file(self, file_id):
        saved_schema_file = self.saved_files[file_id]
        saved_yaml_dict = saved_schema_file.dict_from_yaml
        new_yaml_dict = {}
        self.handle_schema_file_changes(saved_schema_file, saved_yaml_dict, new_yaml_dict)
        self.saved_manifest.files.pop(file_id)

    # For each key in a schema file dictionary, process the changed, deleted, and added
    # elemnts for the key lists
    def handle_schema_file_changes(self, schema_file, saved_yaml_dict, new_yaml_dict):
        # loop through comparing previous dict_from_yaml with current dict_from_yaml
        # Need to do the deleted/added/changed thing, just like the files lists

        env_var_changes = {}
        if schema_file.file_id in self.env_vars_changed_schema_files:
            env_var_changes = self.env_vars_changed_schema_files[schema_file.file_id]

        # models, seeds, snapshots, analyses
        for dict_key in ["models", "seeds", "snapshots", "analyses"]:
            key_diff = self.get_diff_for(dict_key, saved_yaml_dict, new_yaml_dict)
            if key_diff["changed"]:
                for elem in key_diff["changed"]:
                    self.delete_schema_mssa_links(schema_file, dict_key, elem)
                    self.merge_patch(schema_file, dict_key, elem, True)
            if key_diff["deleted"]:
                for elem in key_diff["deleted"]:
                    self.delete_schema_mssa_links(schema_file, dict_key, elem)
            if key_diff["added"]:
                for elem in key_diff["added"]:
                    self.merge_patch(schema_file, dict_key, elem, True)
            # Handle schema file updates due to env_var changes
            if dict_key in env_var_changes and dict_key in new_yaml_dict:
                for name in env_var_changes[dict_key]:
                    if name in key_diff["changed_or_deleted_names"]:
                        continue
                    elem = self.get_schema_element(new_yaml_dict[dict_key], name)
                    if elem:
                        self.delete_schema_mssa_links(schema_file, dict_key, elem)
                        self.merge_patch(schema_file, dict_key, elem, True)

        # sources
        dict_key = "sources"
        source_diff = self.get_diff_for(dict_key, saved_yaml_dict, new_yaml_dict)
        if source_diff["changed"]:
            for source in source_diff["changed"]:
                if "overrides" in source:  # This is a source patch; need to re-parse orig source
                    self.remove_source_override_target(source)
                self.delete_schema_source(schema_file, source)
                self.merge_patch(schema_file, dict_key, source, True)
        if source_diff["deleted"]:
            for source in source_diff["deleted"]:
                if "overrides" in source:  # This is a source patch; need to re-parse orig source
                    self.remove_source_override_target(source)
                self.delete_schema_source(schema_file, source)
        if source_diff["added"]:
            for source in source_diff["added"]:
                if "overrides" in source:  # This is a source patch; need to re-parse orig source
                    self.remove_source_override_target(source)
                self.merge_patch(schema_file, dict_key, source, True)
        # Handle schema file updates due to env_var changes
        if dict_key in env_var_changes and dict_key in new_yaml_dict:
            for name in env_var_changes[dict_key]:
                if name in source_diff["changed_or_deleted_names"]:
                    continue
                source = self.get_schema_element(new_yaml_dict[dict_key], name)
                if source:
                    if "overrides" in source:
                        self.remove_source_override_target(source)
                    self.delete_schema_source(schema_file, source)
                    self.merge_patch(schema_file, dict_key, source, True)

        def handle_change(key: str, delete: Callable):
            self._handle_element_change(
                schema_file, saved_yaml_dict, new_yaml_dict, env_var_changes, key, delete
            )

        handle_change("macros", self.delete_schema_macro_patch)
        handle_change("exposures", self.delete_schema_exposure)
        handle_change("metrics", self.delete_schema_metric)
        handle_change("groups", self.delete_schema_group)
        handle_change("semantic_models", self.delete_schema_semantic_model)
        handle_change("saved_queries", self.delete_schema_saved_query)

    def _handle_element_change(
        self, schema_file, saved_yaml_dict, new_yaml_dict, env_var_changes, dict_key: str, delete
    ):
        element_diff = self.get_diff_for(dict_key, saved_yaml_dict, new_yaml_dict)
        if element_diff["changed"]:
            for element in element_diff["changed"]:
                delete(schema_file, element)
                self.merge_patch(schema_file, dict_key, element, True)
        if element_diff["deleted"]:
            for element in element_diff["deleted"]:
                delete(schema_file, element)
        if element_diff["added"]:
            for element in element_diff["added"]:
                self.merge_patch(schema_file, dict_key, element, True)
        # Handle schema file updates due to env_var changes
        if dict_key in env_var_changes and dict_key in new_yaml_dict:
            for name in env_var_changes[dict_key]:
                if name in element_diff["changed_or_deleted_names"]:
                    continue
                elem = self.get_schema_element(new_yaml_dict[dict_key], name)
                if elem:
                    delete(schema_file, elem)
                    self.merge_patch(schema_file, dict_key, elem, True)

    # Take a "section" of the schema file yaml dictionary from saved and new schema files
    # and determine which parts have changed
    def get_diff_for(self, key, saved_yaml_dict, new_yaml_dict):
        if key in saved_yaml_dict or key in new_yaml_dict:
            saved_elements = saved_yaml_dict[key] if key in saved_yaml_dict else []
            new_elements = new_yaml_dict[key] if key in new_yaml_dict else []
        else:
            return {"deleted": [], "added": [], "changed": []}
        # for each set of keys, need to create a dictionary of names pointing to entry
        saved_elements_by_name = {}
        new_elements_by_name = {}
        # sources have two part names?
        for element in saved_elements:
            saved_elements_by_name[element["name"]] = element
        for element in new_elements:
            new_elements_by_name[element["name"]] = element

        # now determine which elements, by name, are added, deleted or changed
        saved_element_names = set(saved_elements_by_name.keys())
        new_element_names = set(new_elements_by_name.keys())
        deleted = saved_element_names.difference(new_element_names)
        added = new_element_names.difference(saved_element_names)
        common = saved_element_names.intersection(new_element_names)
        changed = []
        for element_name in common:
            if saved_elements_by_name[element_name] != new_elements_by_name[element_name]:
                changed.append(element_name)

        # make lists of yaml elements to return as diffs
        deleted_elements = [saved_elements_by_name[name].copy() for name in deleted]
        added_elements = [new_elements_by_name[name].copy() for name in added]
        changed_elements = [new_elements_by_name[name].copy() for name in changed]

        diff = {
            "deleted": deleted_elements,
            "added": added_elements,
            "changed": changed_elements,
            "changed_or_deleted_names": list(changed) + list(deleted),
        }
        return diff

    # Merge a patch file into the pp_dict in a schema file. The "new_patch"
    # flag indicates that we're processing a schema file, so if a matching
    # patch has already been scheduled, replace it.
    def merge_patch(self, schema_file, key, patch, new_patch=False):
        if schema_file.pp_dict is None:
            schema_file.pp_dict = {}
        pp_dict = schema_file.pp_dict
        if key not in pp_dict:
            pp_dict[key] = [patch]
        else:
            # check that this patch hasn't already been saved
            found_elem = None
            for elem in pp_dict[key]:
                if elem["name"] == patch["name"]:
                    found_elem = elem
            if not found_elem:
                pp_dict[key].append(patch)
            elif found_elem and new_patch:
                # remove patch and replace with new one
                pp_dict[key].remove(found_elem)
                pp_dict[key].append(patch)

        schema_file.delete_from_env_vars(key, patch["name"])
        self.add_to_pp_files(schema_file)

    # For model, seed, snapshot, analysis schema dictionary keys,
    # delete the patches and tests from the patch
    def delete_schema_mssa_links(self, schema_file, dict_key, elem):
        # find elem node unique_id in node_patches
        prefix = key_to_prefix[dict_key]
        elem_unique_ids = []
        for unique_id in schema_file.node_patches:
            if not unique_id.startswith(prefix):
                continue
            parts = unique_id.split(".")
            elem_name = parts[2]
            if elem_name == elem["name"]:
                elem_unique_ids.append(unique_id)

        # remove elem node and remove unique_id from node_patches
        for elem_unique_id in elem_unique_ids:
            # might have been already removed
            if (
                elem_unique_id in self.saved_manifest.nodes
                or elem_unique_id in self.saved_manifest.disabled
            ):
                if elem_unique_id in self.saved_manifest.nodes:
                    nodes = [self.saved_manifest.nodes.pop(elem_unique_id)]
                else:
                    # The value of disabled items is a list of nodes
                    nodes = self.saved_manifest.disabled.pop(elem_unique_id)
                # need to add the node source_file to pp_files
                for node in nodes:
                    file_id = node.file_id
                    # need to copy new file to saved files in order to get content
                    if file_id in self.new_files:
                        self.saved_files[file_id] = deepcopy(self.new_files[file_id])
                    if self.saved_files[file_id]:
                        source_file = self.saved_files[file_id]
                        self.add_to_pp_files(source_file)
                    # if the node's group has changed - need to reparse all referencing nodes to ensure valid ref access
                    if node.group != elem.get("group"):
                        self.schedule_referencing_nodes_for_parsing(node.unique_id)
                    # if the node's latest version has changed - need to reparse all referencing nodes to ensure correct ref resolution
                    if node.is_versioned and node.latest_version != elem.get("latest_version"):
                        self.schedule_referencing_nodes_for_parsing(node.unique_id)
            # remove from patches
            schema_file.node_patches.remove(elem_unique_id)

        # for models, seeds, snapshots (not analyses)
        if dict_key in ["models", "seeds", "snapshots"]:
            # find related tests and remove them
            self.remove_tests(schema_file, dict_key, elem["name"])

    def remove_tests(self, schema_file, dict_key, name):
        tests = schema_file.get_tests(dict_key, name)
        for test_unique_id in tests:
            if test_unique_id in self.saved_manifest.nodes:
                self.saved_manifest.nodes.pop(test_unique_id)
        schema_file.remove_tests(dict_key, name)

    def delete_schema_source(self, schema_file, source_dict):
        # both patches, tests, and source nodes
        source_name = source_dict["name"]
        # There may be multiple sources for each source dict, since
        # there will be a separate source node for each table.
        # SourceDefinition name = table name, dict name is source_name
        sources = schema_file.sources.copy()
        for unique_id in sources:
            if unique_id in self.saved_manifest.sources:
                source = self.saved_manifest.sources[unique_id]
                if source.source_name == source_name:
                    source = self.saved_manifest.sources.pop(unique_id)
                    schema_file.sources.remove(unique_id)
                    self.schedule_referencing_nodes_for_parsing(unique_id)

        self.remove_tests(schema_file, "sources", source_name)

    def delete_schema_macro_patch(self, schema_file, macro):
        # This is just macro patches that need to be reapplied
        macro_unique_id = None
        if macro["name"] in schema_file.macro_patches:
            macro_unique_id = schema_file.macro_patches[macro["name"]]
            del schema_file.macro_patches[macro["name"]]
        if macro_unique_id and macro_unique_id in self.saved_manifest.macros:
            macro = self.saved_manifest.macros.pop(macro_unique_id)
            macro_file_id = macro.file_id
            if macro_file_id in self.new_files:
                self.saved_files[macro_file_id] = deepcopy(self.new_files[macro_file_id])
                self.add_to_pp_files(self.saved_files[macro_file_id])

    # exposures are created only from schema files, so just delete
    # the exposure or the disabled exposure.
    def delete_schema_exposure(self, schema_file, exposure_dict):
        exposure_name = exposure_dict["name"]
        exposures = schema_file.exposures.copy()
        for unique_id in exposures:
            if unique_id in self.saved_manifest.exposures:
                exposure = self.saved_manifest.exposures[unique_id]
                if exposure.name == exposure_name:
                    self.saved_manifest.exposures.pop(unique_id)
                    schema_file.exposures.remove(unique_id)
            elif unique_id in self.saved_manifest.disabled:
                self.delete_disabled(unique_id, schema_file.file_id)

    # groups are created only from schema files, so just delete the group
    def delete_schema_group(self, schema_file, group_dict):
        group_name = group_dict["name"]
        groups = schema_file.groups.copy()
        for unique_id in groups:
            if unique_id in self.saved_manifest.groups:
                group = self.saved_manifest.groups[unique_id]
                if group.name == group_name:
                    self.schedule_nodes_for_parsing(self.saved_manifest.group_map[group.name])
                    self.saved_manifest.groups.pop(unique_id)
                    schema_file.groups.remove(unique_id)

    # metrics are created only from schema files, but also can be referred to by other nodes
    def delete_schema_metric(self, schema_file, metric_dict):
        metric_name = metric_dict["name"]
        metrics = schema_file.metrics.copy()
        for unique_id in metrics:
            if unique_id in self.saved_manifest.metrics:
                metric = self.saved_manifest.metrics[unique_id]
                if metric.name == metric_name:
                    # Need to find everything that referenced this metric and schedule for parsing
                    if unique_id in self.saved_manifest.child_map:
                        self.schedule_nodes_for_parsing(self.saved_manifest.child_map[unique_id])
                    self.saved_manifest.metrics.pop(unique_id)
                    schema_file.metrics.remove(unique_id)
            elif unique_id in self.saved_manifest.disabled:
                self.delete_disabled(unique_id, schema_file.file_id)

    def delete_schema_saved_query(self, schema_file, saved_query_dict):
        saved_query_name = saved_query_dict["name"]
        saved_queries = schema_file.saved_queries.copy()
        for unique_id in saved_queries:
            if unique_id in self.saved_manifest.saved_queries:
                saved_query = self.saved_manifest.saved_queries[unique_id]
                if saved_query.name == saved_query_name:
                    # Need to find everything that referenced this saved_query and schedule for parsing
                    if unique_id in self.saved_manifest.child_map:
                        self.schedule_nodes_for_parsing(self.saved_manifest.child_map[unique_id])
                    self.saved_manifest.saved_queries.pop(unique_id)
            elif unique_id in self.saved_manifest.disabled:
                self.delete_disabled(unique_id, schema_file.file_id)

    def delete_schema_semantic_model(self, schema_file, semantic_model_dict):
        semantic_model_name = semantic_model_dict["name"]
        semantic_models = schema_file.semantic_models.copy()
        for unique_id in semantic_models:
            if unique_id in self.saved_manifest.semantic_models:
                semantic_model = self.saved_manifest.semantic_models[unique_id]
                if semantic_model.name == semantic_model_name:
                    # Need to find everything that referenced this semantic model and schedule for parsing
                    if unique_id in self.saved_manifest.child_map:
                        self.schedule_nodes_for_parsing(self.saved_manifest.child_map[unique_id])
                    self.saved_manifest.semantic_models.pop(unique_id)
                    schema_file.semantic_models.remove(unique_id)
            elif unique_id in self.saved_manifest.disabled:
                self.delete_disabled(unique_id, schema_file.file_id)

        metrics = schema_file.generated_metrics.copy()
        for unique_id in metrics:
            if unique_id in self.saved_manifest.metrics:
                self.saved_manifest.metrics.pop(unique_id)
                schema_file.generated_metrics.remove(unique_id)
            elif unique_id in self.saved_manifest.disabled:
                self.delete_disabled(unique_id, schema_file.file_id)

    def get_schema_element(self, elem_list, elem_name):
        for element in elem_list:
            if "name" in element and element["name"] == elem_name:
                return element
        return None

    def get_schema_file_for_source(self, package_name, source_name):
        schema_file = None
        for source in self.saved_manifest.sources.values():
            if source.package_name == package_name and source.source_name == source_name:
                file_id = source.file_id
                if file_id in self.saved_files:
                    schema_file = self.saved_files[file_id]
                break
        return schema_file

    def get_source_override_file_and_dict(self, source):
        package = source["overrides"]
        source_name = source["name"]
        orig_source_schema_file = self.get_schema_file_for_source(package, source_name)
        orig_sources = orig_source_schema_file.dict_from_yaml["sources"]
        orig_source = self.get_schema_element(orig_sources, source_name)
        return (orig_source_schema_file, orig_source)

    def remove_source_override_target(self, source_dict):
        (orig_file, orig_source) = self.get_source_override_file_and_dict(source_dict)
        if orig_source:
            self.delete_schema_source(orig_file, orig_source)
            self.merge_patch(orig_file, "sources", orig_source)
            self.add_to_pp_files(orig_file)

    # This builds a dictionary of files that need to be scheduled for parsing
    # because the env var has changed.
    # source_files
    #   env_vars_changed_source_files: [file_id, file_id...]
    # schema_files
    #   env_vars_changed_schema_files: {file_id: {"yaml_key": [name, ..]}}
    def build_env_vars_to_files(self):
        unchanged_vars = []
        changed_vars = []
        delete_vars = []
        # Check whether the env_var has changed and add it to
        # an unchanged or changed list
        for env_var in self.saved_manifest.env_vars:
            prev_value = self.saved_manifest.env_vars[env_var]
            current_value = os.getenv(env_var)
            if current_value is None:
                # This will be true when depending on the default value.
                # We store env vars set by defaults as a static string so we can recognize they have
                # defaults.  We depend on default changes triggering reparsing by file change. If
                # the file has not changed we can assume the default has not changed.
                if prev_value == DEFAULT_ENV_PLACEHOLDER:
                    unchanged_vars.append(env_var)
                    continue
                # env_var no longer set, remove from manifest
                delete_vars.append(env_var)
            if prev_value == current_value:
                unchanged_vars.append(env_var)
            else:  # prev_value != current_value
                changed_vars.append(env_var)
        for env_var in delete_vars:
            del self.saved_manifest.env_vars[env_var]

        env_vars_changed_source_files = []
        env_vars_changed_schema_files = {}
        # The SourceFiles contain a list of env_vars that were used in the file.
        # The SchemaSourceFiles contain a dictionary of yaml_key to schema entry names to
        # a list of vars.
        # Create a list of file_ids for source_files that need to be reparsed, and
        # a dictionary of file_ids to yaml_keys to names.
        for source_file in self.saved_files.values():
            file_id = source_file.file_id
            if not source_file.env_vars:
                continue
            if source_file.parse_file_type == ParseFileType.Schema:
                for yaml_key in source_file.env_vars.keys():
                    for name in source_file.env_vars[yaml_key].keys():
                        for env_var in source_file.env_vars[yaml_key][name]:
                            if env_var in changed_vars:
                                if file_id not in env_vars_changed_schema_files:
                                    env_vars_changed_schema_files[file_id] = {}
                                if yaml_key not in env_vars_changed_schema_files[file_id]:
                                    env_vars_changed_schema_files[file_id][yaml_key] = []
                                if name not in env_vars_changed_schema_files[file_id][yaml_key]:
                                    env_vars_changed_schema_files[file_id][yaml_key].append(name)
                                break  # if one env_var is changed we can stop

            else:
                for env_var in source_file.env_vars:
                    if env_var in changed_vars:
                        env_vars_changed_source_files.append(file_id)
                        break  # if one env_var is changed we can stop

        return (env_vars_changed_source_files, env_vars_changed_schema_files)
