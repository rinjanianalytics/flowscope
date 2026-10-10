# Install FlowScope

*FlowScope* is written in Rust. It provides its 4 components in a single binary
or Docker image. It also requires [Kafka](https://kafka.apache.org/quickstart)
(1.0+) and [ClickHouse](https://clickhouse.com/docs/en/getting-started/install/)
(22.4+).

If you only want to try *FlowScope*, follow the [tutorial](02-collect.md)
instead. It uses the `docker compose` setup and covers the first configuration
steps.

## Requirements

*FlowScope* itself does not need much memory and disk space, but Kafka and
ClickHouse do. The recommended configuration for the complete setup is:

- 4 vCPUs (AMD64 or ARM64)
- 8 GB of RAM, 32 GB or more is better
- 100 GB of disk, 50 GB is the minimum

## Docker image

There is no published image yet. Build it from the repository, with
[Docker](https://docs.docker.com/engine/install/):

```console
# git clone https://github.com/rinjanianalytics/flowscope
# cd flowscope
# docker build -f docker/Dockerfile -t ghcr.io/rinjanianalytics/flowscope:main .
# docker run --rm ghcr.io/rinjanianalytics/flowscope:main help
```

Check the `docker/docker-compose.yml` file for an example of how to deploy
*FlowScope* using containers. `docker compose up` builds the image when it is
missing. If you want to use `docker compose`, see the
[tutorial](02-collect.md). This documentation assumes you are using the `docker
compose` setup.

The minimum supported version for Docker Engine is v23.
Check with `docker version -f {{ .Server.Version }}`.

## Compilation from source

You need [Rust](https://www.rust-lang.org/tools/install) (the version is in
`rust-toolchain.toml`), CMake and a C compiler for the Kafka client, and
[NodeJS](https://nodejs.org/en/download/) (22+) with
[pnpm](https://pnpm.io/installation) for the web interface. For example, on
Debian:

```console
# apt install cmake build-essential nodejs npm
# npm install -g pnpm
# curl -sSf https://sh.rustup.rs | sh
```

Build the web interface first: the binary embeds it.

```console
# cd web
# pnpm install
# pnpm build
# cd ..
# cargo build --release
```

The resulting executable is `target/release/flowscope`. Without the first
step, the binary still works but its console has no web interface.

The checks are:

- `cargo test --workspace` to run the tests
- `cargo clippy --workspace --all-targets` to lint the Rust code
- `cargo fmt --all` to format the Rust code
- `pnpm lint` and `pnpm test`, in `web/`, for the web interface

## Upgrade

Read the [changelog](99-changelog.md) before you upgrade.
Upgrade the orchestrator first. This will update the ClickHouse database if
needed. Then, upgrade all inlets and outlets. Then the console.

When using `docker compose`, use the following commands to update your
installation.

```console
# cd flowscope
# git pull
# docker compose build
# docker compose pull --ignore-buildable
# docker compose stop flowscope-orchestrator
# docker compose up --wait
```

Your changes to `.env`, `config/` and `docker/docker-compose-local.yml` are
kept by Git. Fix the conflicts it reports, if any.

> [!NOTE]
> The upgrade can be done from any version to any more recent version. Downgrading is not supported.
