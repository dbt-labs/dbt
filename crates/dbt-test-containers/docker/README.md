# Test Docker Containers

This directory contains a set of Dockerfiles to aid in running tests in isolated environments. These images are used by tests like `test_dbt_compile`, which leverages `dbt/Dockerfile` to run dbt commands in an isolated python environment.

## Building Docker Images

This is handled using the `bollard` crate automatically when running tests. Tests call `initialize_container` (see: `/tests/common/container/docker.rs`) to build and start the container.