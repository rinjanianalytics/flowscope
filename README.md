# FlowScope: flow collector, enricher and visualizer

**FlowScope is a Rust rewrite of [Akvorado](https://github.com/akvorado/akvorado).**
Akvorado is open source software written in Go and developed by Free Mobile.
It is the origin of this project and its inspiration: the design, the data
model and the documentation come from it. FlowScope is built by Rinjani
Analytics and is not affiliated with Akvorado or Free Mobile.

FlowScope receives network flows (NetFlow v5/v9, IPFIX and sFlow), enriches
them with interface names (SNMP or gNMI), routing information (BMP or BioRIS)
and geographic information, and stores them in ClickHouse through Kafka. A web
console lets you explore the collected flows with time series and sankey
graphs.

It keeps the configuration format, the ClickHouse schema, the Kafka messages
and the HTTP API of Akvorado, so both can run side by side and a deployment
can move from one to the other.

## Status

The backend services are ported: inlet, outlet, orchestrator and console API.
The web interface, the SDK, the demo exporter and the packaging are in
progress. See [RUST_PORT_PLAN.md](RUST_PORT_PLAN.md) for the progress and the
differences with Akvorado.

## Build

FlowScope needs Rust (see `rust-toolchain.toml`) and CMake for the Kafka
client.

```console
$ cargo build --release
$ target/release/flowscope version
```

The `flowscope` binary runs every service: `flowscope orchestrator`,
`flowscope inlet`, `flowscope outlet` and `flowscope console`, each with a
configuration file or the URL of the orchestrator.

## Compatibility with Akvorado

Some names are kept from Akvorado on purpose, so existing deployments keep
working:

- metrics are prefixed with `akvorado_`, so dashboards and alerts still apply,
- configuration overrides use `AKVORADO_CFG_*` environment variables,
- the outlet uses the `akvorado-outlet` Kafka consumer group, so it resumes
  from the offsets of an Akvorado outlet,
- the ClickHouse tables, the dictionaries and the Kafka topics have the same
  names.

## Repository layout

- `crates/`: the Rust workspace (`flowscope` binary and `flowscope-*`
  libraries),
- `console/data/docs`: the documentation, served by the console,
- `tests/golden`: data dumped from Akvorado, used to check FlowScope gives
  the same results,
- the Go source of Akvorado is kept as the reference for these checks.

## License

FlowScope is licensed under the [AGPLv3 license](LICENSE.txt), like Akvorado.

## Credits

FlowScope would not exist without [Akvorado](https://github.com/akvorado/akvorado)
and its authors. The Go source of Akvorado is kept in this repository, with its
history, as the reference the Rust code is checked against. The files ported
from Akvorado keep the copyright notice of Free Mobile.
