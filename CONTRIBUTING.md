# Contributing to FlowScope

FlowScope is a Rust rewrite of [Akvorado](https://github.com/akvorado/akvorado).
A problem which also exists in Akvorado is best reported there too.

## Getting the code to build

You need Rust (the version is in `rust-toolchain.toml`), CMake and a C
compiler, and Node.js 22 or later with pnpm.

```console
$ cd web && pnpm install && pnpm build && cd ..
$ cargo build
```

The first command builds the web interface, which the console embeds. Without
it, the binary works but the console has no web interface.

## Checks

Run these before you send a change. The CI runs the same ones.

```console
$ cargo fmt --all --check
$ cargo clippy --workspace --all-targets --all-features -- -D warnings
$ cargo test --workspace --all-features
$ cd web && pnpm lint && pnpm test
```

Some tests need a service and are skipped by default. `tests/interop` has a
Docker Compose file starting Kafka, ClickHouse, Redis and an SNMP agent for
them:

```console
$ docker compose -f tests/interop/docker-compose.yml up -d
$ cargo test --workspace -- --ignored
```

## Working on the web interface

`pnpm dev`, in `web/`, serves the interface with live reload. It needs a
console for its API: set `FLOWSCOPE_API_URL` to its address (the default is
`http://127.0.0.1:8080`).

```console
$ FLOWSCOPE_API_URL=http://127.0.0.1:8081 pnpm dev
```

The types of the API come from the OpenAPI document of the console. After a
change to the API types in `crates/flowscope-sdk`, update both:

```console
$ FLOWSCOPE_UPDATE_OPENAPI=1 cargo test -p rinjani-flowscope-sdk
$ cd web && pnpm run api
```

## Branches and pull requests

Nothing is pushed to `main`. Every change comes from a branch, through a pull
request:

1. Create a branch: `git switch -c outlet/fix-something`.
2. Push it and open a pull request to `main`.
3. The `test` and `web` checks have to pass, on a branch up to date with
   `main`.
4. The owner of the repository reviews, tries the change and approves.
5. The owner merges. `main` is what gets built and released.

GitHub enforces this with two rule sets, kept in `.github/rulesets`. The first
one applies to everyone: a pull request and passing checks, no force push, no
deletion of `main`. The second one asks for the approval of the owner
(`.github/CODEOWNERS`). The owner cannot approve their own pull request, so
they can merge it without one, but still not push to `main`.

To apply the rule sets to a repository:

```console
$ gh api -X POST repos/OWNER/REPO/rulesets --input .github/rulesets/main-checks.json
$ gh api -X POST repos/OWNER/REPO/rulesets --input .github/rulesets/main-review.json
```

## Packages

The Cargo packages carry the `rinjani-` prefix: `rinjani-flowscope` for the
binary, `rinjani-flowscope-sdk` for the client library. The libraries keep
their short names in the code (`flowscope_sdk`).

The console package embeds the web interface, which is not in git. Build it
before you package, or the published console has no interface:

```console
$ cd web && pnpm install && pnpm build && cd ..
$ cargo package --workspace
```

`cargo package` builds each package from its archive. Remove
`target/package` first if it fails with "no hash listed".

## Compatibility with Akvorado

FlowScope reads the configuration of Akvorado and shares its ClickHouse
tables, its Kafka messages and its HTTP API. `tests/golden/testdata` holds
answers recorded from Akvorado: SQL queries, decoded flows, parsed filters,
HTTP answers. The tests compare FlowScope with them. A change which makes one
of these tests fail breaks this compatibility: say so, and why it is worth it.

## Style

- Comments say why, not what.
- User-facing text and documentation use short sentences and plain words.
- Commit messages start with the part touched: `outlet: ...`, `web: ...`,
  `docs: ...`.

## License

By contributing, you accept that your contribution is licensed under the
[AGPLv3 license](LICENSE.txt).
