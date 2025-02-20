#!/usr/bin/env python
import os
import sys

if sys.version_info < (3, 9):
    print("Error: dbt does not support this version of Python.")
    print("Please upgrade to Python 3.9 or higher.")
    sys.exit(1)


from setuptools import setup

try:
    from setuptools import find_namespace_packages
except ImportError:
    # the user has a downlevel version of setuptools.
    print("Error: dbt requires setuptools v40.1.0 or higher.")
    print('Please upgrade setuptools with "pip install --upgrade setuptools" ' "and try again")
    sys.exit(1)


this_directory = os.path.abspath(os.path.dirname(__file__))
with open(os.path.join(this_directory, "README.md")) as f:
    long_description = f.read()


package_name = "dbt-core"
package_version = "1.10.0a1.dev02202025"
description = """With dbt, data analysts and engineers can build analytics \
the way engineers build applications."""


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
    include_package_data=True,
    test_suite="test",
    entry_points={
        "console_scripts": ["dbt = dbt.cli.main:cli"],
    },
    install_requires=[
        # ----
        # dbt-core uses these packages deeply, throughout the codebase, and there have been breaking changes in past patch releases (even though these are major-version-one).
        # Pin to the patch or minor version, and bump in each new minor version of dbt-core.
        "agate>=1.7.0,<1.10",
        "Jinja2>=3.1.3,<4",
        "mashumaro[msgpack]>=3.9,<3.15",
        # ----
        # dbt-core uses these packages in standard ways. Pin to the major version, and check compatibility
        # with major versions in each new minor version of dbt-core.
        "click>=8.0.2,<9.0",
        "networkx>=2.3,<4.0",
        "protobuf>=5.0,<6.0",
        "requests<3.0.0",  # should match dbt-common
        "snowplow-tracker>=1.0.2,<2.0",
        # ----
        # These packages are major-version-0. Keep upper bounds on upcoming minor versions (which could have breaking changes)
        # and check compatibility / bump in each new minor version of dbt-core.
        "pathspec>=0.9,<0.13",
        "sqlparse>=0.5.0,<0.6.0",
        # ----
        # These are major-version-0 packages also maintained by dbt-labs.
        # Accept patches but avoid automatically updating past a set minor version range.
        "dbt-extractor>=0.5.0,<=0.6",
        "dbt-semantic-interfaces>=0.8.3,<0.9",
        # Minor versions for these are expected to be backwards-compatible
        "dbt-common>=1.15.0,<2.0",
        "dbt-adapters>=1.13.0,<2.0",
        # ----
        # Expect compatibility with all new versions of these packages, so lower bounds only.
        "packaging>20.9",
        "pytz>=2015.7",
        "pyyaml>=6.0",
        "daff>=1.3.46",
        "typing-extensions>=4.4",
        # ----
    ],
    zip_safe=False,
    classifiers=[
        "Development Status :: 5 - Production/Stable",
        "License :: OSI Approved :: Apache Software License",
        "Operating System :: Microsoft :: Windows",
        "Operating System :: MacOS :: MacOS X",
        "Operating System :: POSIX :: Linux",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
    ],
    python_requires=">=3.9",
)
