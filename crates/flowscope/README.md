# rinjani-flowscope

FlowScope is the flow collector of Rinjani Analytics. It receives NetFlow,
IPFIX and sFlow, enriches the flows, stores them in ClickHouse and shows them
in a web interface. FlowScope is a Rust rewrite of [Akvorado][].

The package is `rinjani-flowscope`. It installs the `flowscope` binary.

```console
$ cargo install rinjani-flowscope
$ flowscope version
```

The build needs CMake and a C compiler, for the Kafka client. The web
interface is already in the package: Node.js is not needed.

The binary runs every service: `flowscope orchestrator`, `flowscope inlet`,
`flowscope outlet`, `flowscope console` and `flowscope demo-exporter`. A
working setup also needs Kafka and ClickHouse. The [repository][] has a
Docker Compose setup with all of it, and the documentation.

## License

AGPL-3.0-only.

[repository]: https://github.com/rinjanianalytics/flowscope
[Akvorado]: https://github.com/akvorado/akvorado
