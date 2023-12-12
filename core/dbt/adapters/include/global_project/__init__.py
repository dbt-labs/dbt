import os

PACKAGE_PATH = os.path.dirname(__file__)
PROJECT_NAME = "dbt"


def get_global_project_path() -> str:
    return PACKAGE_PATH


def get_global_project_name() -> str:
    return PROJECT_NAME
