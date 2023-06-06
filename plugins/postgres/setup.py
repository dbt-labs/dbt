#!/usr/bin/env python
import os
import sys

if sys.version_info < (3, 8):
    print("Error: dbt does not support this version of Python.")
    print("Please upgrade to Python 3.8 or higher.")
    sys.exit(1)


from setuptools import setup

try:
    from setuptools import find_namespace_packages
except ImportError:
    # the user has a downlevel version of setuptools.
    print("Error: dbt requires setuptools v40.1.0 or higher.")
    print('Please upgrade setuptools with "pip install --upgrade setuptools" ' "and try again")
    sys.exit(1)


PSYCOPG2_MESSAGE = """
No package name override was set.
Using 'psycopg2-binary' package to satisfy 'psycopg2'

If you experience segmentation faults, silent crashes, or installation errors,
consider retrying with the 'DBT_PSYCOPG2_NAME' environment variable set to
'psycopg2'. It may require a compiler toolchain and development libraries!
""".strip()


def _dbt_psycopg2_name():
    # if the user chose something, use that
    package_name = os.getenv("DBT_PSYCOPG2_NAME", "")
    if package_name:
        return package_name

    # default to psycopg2-binary for all OSes/versions
    print(PSYCOPG2_MESSAGE)
    return "psycopg2-binary"


package_name = "dbt-postgres"
package_version = "1.6.0b2"
description = """The postgres adapter plugin for dbt (data build tool)"""

this_directory = os.path.abspath(os.path.dirname(__file__))
with open(os.path.join(this_directory, "README.md")) as f:
    long_description = f.read()

DBT_PSYCOPG2_NAME = _dbt_psycopg2_name()

setup(
    name=package_name,
    version=package_version,
    description=description,
    long_description=long_description,
    long_description_content_type="text/markdown",
    author="dbt Labs",
    author_email="info@dbtlabs.com",
    url="https://github.com/dbt-labs/dbt-core",
    packages=find_namespace_packages(include=["dbt", "dbt.*"]),
    package_data={
        "dbt": [
            "include/postgres/dbt_project.yml",
            "include/postgres/sample_profiles.yml",
            "include/postgres/macros/*.sql",
            "include/postgres/macros/**/*.sql",
        ]
    },
    install_requires=[
        "dbt-core=={}".format(package_version),
        "{}~=2.8".format(DBT_PSYCOPG2_NAME),
        # installed via dbt-core, but referenced directly, don't pin to avoid version conflicts with dbt-core
        "agate",
    ],
    zip_safe=False,
    classifiers=[
        "Development Status :: 5 - Production/Stable",
        "License :: OSI Approved :: Apache Software License",
        "Operating System :: Microsoft :: Windows",
        "Operating System :: MacOS :: MacOS X",
        "Operating System :: POSIX :: Linux",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
    ],
    python_requires=">=3.8",
)
