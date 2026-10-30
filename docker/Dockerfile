ARG py_version=3.11.2

FROM python:$py_version-slim-bullseye AS base

RUN apt-get update \
  # Install version-pinned packages for reproducible builds
  && apt-get install -y --no-install-recommends \
    build-essential=12.9 \
    ca-certificates=20230311+deb12u1~deb11u1 \
    libpq-dev=13.23-0+deb11u4 \
    make=4.3-4.1 \
    openssh-client=1:8.4p1-5+deb11u7 \
    software-properties-common=0.96.20.2-2.1 \
  # Prevent dist-upgrade from changing pinned package versions
  && apt-mark hold \
    build-essential \
    ca-certificates \
    libpq-dev \
    make \
    openssh-client \
    software-properties-common \
  # Apply security updates to non-pinned base image packages
  && apt-get dist-upgrade -y \
  # Install temporary dependencies for building git from source (removed later)
  && apt-get install -y --no-install-recommends \
    wget \
    libcurl4-gnutls-dev \
    libexpat1-dev \
    libssl-dev \
    zlib1g-dev \
  # Build and install git from source to get 2.50.1 since debian only ships with 2.47 for now
  && wget https://github.com/git/git/archive/refs/tags/v2.50.1.tar.gz \
  && tar -xf v2.50.1.tar.gz \
  && cd git-2.50.1 \
  && make prefix=/usr/local NO_TCLTK=1 NO_GETTEXT=1 all \
  && make prefix=/usr/local NO_TCLTK=1 NO_GETTEXT=1 install \
  && cd .. \
  && rm -rf git-2.50.1 v2.50.1.tar.gz \
  # Remove build dependencies to keep image slim
  && apt-get remove -y \
    libcurl4-gnutls-dev \
    libexpat1-dev \
    libssl-dev \
    wget \
    zlib1g-dev \
  && apt-get autoremove -y \
  && apt-get clean \
  && rm -rf \
    /var/lib/apt/lists/* \
    /tmp/* \
    /var/tmp/*

ENV PYTHONIOENCODING=utf-8
ENV LANG=C.UTF-8

RUN python -m pip install --upgrade "pip==24.0" "setuptools==69.2.0" "wheel==0.43.0" --no-cache-dir


FROM base AS dbt-core

ARG commit_ref=main

HEALTHCHECK CMD dbt --version || exit 1

WORKDIR /usr/app/dbt/
ENTRYPOINT ["dbt"]

RUN python -m pip install --no-cache-dir "dbt-core @ git+https://github.com/dbt-labs/dbt-core@${commit_ref}#subdirectory=core"


FROM base AS dbt-postgres

ARG commit_ref=main

HEALTHCHECK CMD dbt --version || exit 1

WORKDIR /usr/app/dbt/
ENTRYPOINT ["dbt"]

RUN python -m pip install --no-cache-dir "dbt-postgres @ git+https://github.com/dbt-labs/dbt-core@${commit_ref}#subdirectory=plugins/postgres"


FROM dbt-core AS dbt-third-party

ARG dbt_third_party

RUN if [ "$dbt_third_party" ]; then \
        python -m pip install --no-cache-dir "${dbt_third_party}"; \
    else \
        echo "No third party adapter provided"; \
    fi \
