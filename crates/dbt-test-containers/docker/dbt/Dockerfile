# This container can be paramaterized with different python and dbt versions
ARG py_version=3.11.2

FROM python:$py_version-slim-bullseye AS base

# Install any additional dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc \
    git \
    && rm -rf /var/lib/apt/lists/*

ARG dbt_version=1.8.3

RUN python -m pip install dbt-snowflake==$dbt_version

# The entrypoint script is responsible for running `dbt deps` and `dbt compile`
COPY entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh

# Output Directory (this directory will be mounted as a volume)
RUN mkdir /output

# DBT Projects should parameterize the dbt_project.yml's `packages-install-path: "{{ env_var('DBT_PACKAGES_INSTALL_PATH') }}"`
ENV DBT_PACKAGES_INSTALL_PATH=/dbt_packages

# Set the working directory to `/project`. This will be mounted by the dbt project.
WORKDIR /project

ENTRYPOINT ["/entrypoint.sh"]