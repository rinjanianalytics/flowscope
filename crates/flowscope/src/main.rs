// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! FlowScope: flow collector, enricher and visualizer. Port of `cmd/akvorado`
//! of Akvorado.

mod console;
mod demoexporter;
mod healthcheck;
mod inlet;
mod orchestrator;
mod outlet;
mod service;
mod version;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "flowscope",
    about = "Flow collector, enricher and visualizer",
    long_about = "FlowScope is a Netflow/IPFIX/sFlow collector. The inlet service receives flows and \
                  pushes them in Kafka, the outlet service decodes, enriches and writes them to \
                  ClickHouse. The orchestrator service configures ClickHouse, Kafka and the other \
                  services. The console service exposes a web interface to look and manipulate the \
                  flows stored inside the ClickHouse database.",
    disable_version_flag = true
)]
struct Cli {
    /// Enable debug logs
    #[arg(short, long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start FlowScope's inlet service
    Inlet(ServiceArgs),
    /// Start FlowScope's outlet service
    Outlet(ServiceArgs),
    /// Start FlowScope's orchestrator service
    Orchestrator(ServiceArgs),
    /// Start FlowScope's console service
    Console(ServiceArgs),
    /// Start FlowScope's exporter simulator service
    DemoExporter(ServiceArgs),
    /// Check healthness
    Healthcheck(healthcheck::Args),
    /// Print version
    Version,
}

/// Arguments common to services.
#[derive(clap::Args)]
struct ServiceArgs {
    /// Configuration file or URL of the orchestrator
    config: Option<String>,
    /// Dump configuration before starting
    #[arg(short = 'D', long)]
    dump: bool,
    /// Check configuration, but does not start
    #[arg(short = 'C', long)]
    check: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    flowscope_common::reporter::setup_logging(cli.debug);
    match cli.command {
        Command::Version => version::run(cli.debug),
        Command::Healthcheck(args) => healthcheck::run(args),
        Command::Inlet(args) => runtime()?.block_on(inlet::run(args)),
        Command::Outlet(args) => runtime()?.block_on(outlet::run(args)),
        Command::Orchestrator(args) => runtime()?.block_on(orchestrator::run(args)),
        Command::Console(args) => runtime()?.block_on(console::run(args)),
        Command::DemoExporter(args) => runtime()?.block_on(demoexporter::run(args)),
    }
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().build()?)
}
