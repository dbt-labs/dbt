# This Dockerfile is used to build a postgres container for running tests
FROM postgres:15.1

ENV POSTGRES_USER=postgres
ENV POSTGRES_PASSWORD=postgres
ENV POSTGRES_DB=postgres

# SQL script that sets up the database for the testing
COPY init.sql /docker-entrypoint-initdb.d/init.sql
