# Internal design

*FlowScope* is written in Rust. It is a rewrite of
[Akvorado](https://github.com/akvorado/akvorado), which is written in Go: each
crate was ported from a Go package and checked against it. This page describes
the Rust code. The [design page](80-design.md) explains what the services do.

## Layout

The code is a Cargo workspace in `crates/`. One binary, `flowscope`, runs every
service. Each service and each building block has its own crate:

| Crate | Role |
|---|---|
| `flowscope` | the binary: one command for each service |
| `flowscope-common` | configuration loading, logs and metrics, task supervision, HTTP server, caches |
| `flowscope-schema` | the columns of a flow, the ClickHouse tables, the encoding of batches |
| `flowscope-pb` | the message the inlet sends to the outlet through Kafka |
| `flowscope-kafka` | the configuration shared by the Kafka clients |
| `flowscope-inlet` | inlet service: receives packets, pushes them to Kafka |
| `flowscope-decoder` | NetFlow v5 and v9, IPFIX and sFlow decoders |
| `flowscope-expr` | the language of the classifier rules |
| `flowscope-snmp` | SNMP v2c and v3 |
| `flowscope-clickhouse` | ClickHouse client, native protocol |
| `flowscope-outlet` | outlet service: decodes, enriches and stores the flows |
| `flowscope-orchestrator` | orchestrator service: configuration, ClickHouse and Kafka setup |
| `flowscope-console` | console service: the API behind the web interface |
| `flowscope-demoexporter` | the simulated router of the demo |
| `flowscope-sdk` | client library for the console API, and its OpenAPI document |

The web interface is in `web/`. The documentation is in `docs/`.

Many source files start with "Port of" and the name of a Go file. It is the
file of Akvorado they come from.

## Services

Each service follows the same steps, in `crates/flowscope/src/`: load the
configuration, create the reporter and the HTTP server, start the components
as tasks, then wait. The `Daemon` of `flowscope-common` supervises the tasks.
When one of them stops or fails, or on `SIGTERM`, it asks all of them to stop
and waits for them. The services run on [Tokio](https://tokio.rs/).

## Configuration

A configuration is a YAML file, or comes from the orchestrator over HTTP. The
loader, in `flowscope-common`, resolves the `!include` tags, applies the YAML
merge keys, puts the file on top of the default values, then applies the
`FLOWSCOPE_CFG_*` environment variables. The result is decoded by a tolerant
deserializer: a key matches whatever its case and its dashes, a single value
is accepted where a list is expected, and a text with commas is split. This
is what lets the configuration files of Akvorado load unchanged.

Each service also carries the migrations of Akvorado: a setting which was
renamed or moved is rewritten before decoding.

## Reporter

The reporter of `flowscope-common` gives logs and metrics to the components.
Logs are JSON lines, written with [tracing](https://docs.rs/tracing). Metrics
are served in the Prometheus text format. Their names start with `flowscope_`,
then the module, like `flowscope_outlet_core_`.

## Flow processing

### Inlet

The inlet does as little as possible. It reads UDP packets and sends them to
Kafka, with the address of the exporter and the time of reception, in a
`RawFlow` protobuf message. Several workers listen on the same port with
`SO_REUSEPORT`. A small eBPF program spreads the packets between them. When the
service is not allowed to load it, the kernel spreads them by itself. The
inlet also asks the kernel for the time each packet arrived and for the number
of packets dropped.

### Outlet

The outlet reads the messages from Kafka, decodes them, adds the data the
packets do not carry, and writes the result to ClickHouse.

The decoders are in `flowscope-decoder`. They are written by hand, with no
dependency, and they never panic on a packet cut short or altered: the tests
feed them thousands of damaged packets. The templates of NetFlow and IPFIX are
kept by exporter and can be saved to a file when the service stops.

The number of workers follows the load: a scaler adds or removes workers from
the time they spend waiting.

## Kafka

Both services use [librdkafka](https://github.com/confluentinc/librdkafka),
through the [rdkafka](https://docs.rs/rdkafka) crate. It is the only large C
dependency: it is built with CMake and linked statically.

## ClickHouse

`flowscope-clickhouse` is a client for the native protocol of ClickHouse,
written for *FlowScope*. The outlet sends the flows by columns, in batches,
which is the fastest way to insert. The console and the orchestrator use the
same client for their queries.

The orchestrator creates and updates the tables. It does not keep a version
number: for each table, view and dictionary, it compares what exists with
what should exist and applies the difference. The result is the same database
as Akvorado, and each one accepts the database of the other.

## Metadata

The outlet needs the name, the description and the speed of the interfaces.
It asks a provider and keeps the answers in a cache, saved to a file when the
service stops. The providers are: static configuration, SNMP (with
`flowscope-snmp`, written for *FlowScope*) and gNMI (with
[tonic](https://docs.rs/tonic)).

## Routing

The BMP provider keeps a copy of the routes of each router. It is a server
written by hand: it parses BMP and BGP messages itself. The routes are stored
in a prefix tree, with their attributes shared between routes, so a full
table fits in memory. The BioRIS provider asks a RIS server over gRPC instead.

## Classifiers

The classifier rules are written in the [expr](https://expr-lang.org/)
language, like in Akvorado. `flowscope-expr` parses and runs the part of the
language the rules use. Regular expressions use the
[regex](https://docs.rs/regex) crate.

## Schema

`flowscope-schema` lists the columns of a flow, with their types and the
tables they belong to. The ClickHouse tables, the batches sent by the outlet
and the dimensions offered by the console all come from this list. To add a
column:

1. add it to the list in `flowscope-schema`, disabled by default if it is
   rarely useful,
2. fill it in the decoders or in the outlet,
3. if the filter language needs a special syntax for it, add it to the grammar
   in `crates/flowscope-console/src/filter/grammar.rs`,
4. document it in `docs/50-configuration.md`.

## Web console

### API

The console is an [axum](https://docs.rs/axum) server. Its routes are under
`/api/v0/console`. The handlers build SQL queries for ClickHouse with a small
query builder, so a value is always quoted.

The types of the API are in `flowscope-sdk`, with the OpenAPI document built
from them. The console serves this document at
`/api/v0/console/openapi.json`. Tests check that the answers recorded from
Akvorado still fit these types.

Users are not logged in by the console. A proxy in front of it does that and
passes the user in HTTP headers.

The saved filters are stored in SQLite, PostgreSQL or MySQL, with
[sqlx](https://docs.rs/sqlx).

### Filter language

The filter language is described by a PEG grammar, the one of Akvorado. The
console does not generate a parser from it: a small interpreter runs the
grammar. It gives the same SQL, the same error messages and the same
completions as Akvorado on more than 2,000 recorded cases.

### Web interface

The web interface is a [Next.js](https://nextjs.org/) application written in
TypeScript, with [shadcn/ui](https://ui.shadcn.com/) components,
[Tailwind CSS](https://tailwindcss.com/), [ECharts](https://echarts.apache.org/)
for the graphs and [CodeMirror](https://codemirror.net/) for the filter
editor. It is exported as static files, which the console embeds and serves.
There is no Node.js server in production.

The files are built for a placeholder path. When the console starts, it
replaces the placeholder with its configured prefix, so the same build works
under any path.

The TypeScript types of the API are generated from the OpenAPI document.

### Documentation

The documentation is embedded in the console too. It is written in Markdown
and rendered with [pulldown-cmark](https://docs.rs/pulldown-cmark).

## Tests

Three kinds of tests keep *FlowScope* in step with Akvorado:

- recorded answers: `tests/golden/testdata` holds what Akvorado answered for
  thousands of inputs (decoded packets, SQL queries, parsed filters, HTTP
  answers). The tests replay them,
- test data: `tests/data` holds the packet captures and the databases of the
  Akvorado tests,
- live tests, skipped by default: they need Kafka, ClickHouse or an SNMP
  agent, started by `tests/interop/docker-compose.yml`.

The recorded answers were made with the Go code, before it was removed from
this repository. They are not made again: they are the reference.
