# Docker for dbt
This docker file is suitable for building dbt Docker images locally or using with CI/CD to automate populating a container registry.


## Building an image:
This Dockerfile can create images for the following targets, each named after the database they support:
* `dbt-core` _(no db-adapter support)_
* `dbt-third-party` _(requires additional build-arg)_

For platform-specific images, please refer to that platform's repository (eg. [dbt-postgres](https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-postgres/docker/README.md))

In order to build a new image, run the following docker command.
```
docker build --tag <your_image_name> --target <target_name> <path/to/dockerfile>
```
---
> **Note:**  Docker must be configured to use [BuildKit](https://docs.docker.com/build/buildkit/) in order for images to build properly!

---

By default the images will be populated with `dbt-core` on `main`.
If you need to use a different version you can specify it by git ref (tag, branch, sha) using the `--build-arg` flag:
```
docker build --tag <your_image_name> \
  --target <target_name> \
  --build-arg commit_ref=<git_ref> \
  <path/to/dockerfile>
```

If you wish to build an image with a third-party adapter you can use the `dbt-third-party` target.
This target requires you provide a path to the adapter that can be processed by `pip` by using the `dbt_third_party` build arg:
```
docker build --tag <your_image_name> \
  --target dbt-third-party \
  --build-arg dbt_third_party=<pip_parsable_install_string> \
  <path/to/dockerfile>
```
This can also be combined with the `commit_ref` build arg to specify a version of `dbt-core`.

### Examples:
To build an image named "my-third-party-dbt" that uses the latest release of [Materialize third party adapter](https://github.com/MaterializeInc/materialize/tree/main/misc/dbt-materialize) and the latest dev version of `dbt-core`:
```
cd dbt-core/docker
docker build --tag my-third-party-dbt \
  --target dbt-third-party \
  --build-arg dbt_third_party=dbt-materialize \
  .
```


## Running an image in a container:
The `ENTRYPOINT` for this Dockerfile is the command `dbt` so you can bind-mount your project to `/usr/app` and use dbt as normal:
```
docker run \
  --network=host \
  --mount type=bind,source=path/to/project,target=/usr/app \
  --mount type=bind,source=path/to/profiles.yml,target=/root/.dbt/profiles.yml \
  my-dbt \
  ls
```
---
**Notes:**
* Bind-mount sources _must_ be an absolute path
* You may need to make adjustments to the docker networking setting depending on the specifics of your data warehouse/database host.

---
