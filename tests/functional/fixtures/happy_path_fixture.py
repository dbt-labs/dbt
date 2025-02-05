import os
from shutil import copytree

import pytest


def delete_files_in_directory(directory_path):
    try:
        with os.scandir(directory_path) as entries:
            for entry in entries:
                if entry.is_file():
                    os.unlink(entry.path)
        print("All files deleted successfully.")
    except OSError:
        print("Error occurred while deleting files.")


@pytest.fixture(scope="class")
def happy_path_project_files(project_root):
    # copy fixture files to the project root
    delete_files_in_directory(project_root)
    copytree(
        os.path.dirname(os.path.realpath(__file__)) + "/happy_path_project", str(project_root)
    )


# We do project_setup first because it will write out a dbt_project.yml.
# This file will be overwritten by the files in happy_path_project later on.
@pytest.fixture(scope="class")
def happy_path_project(project_setup, happy_path_project_files):
    # A fixture that gives functional test the project living in happy_path_project
    return project_setup
