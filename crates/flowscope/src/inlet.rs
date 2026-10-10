// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `inlet` command. Port of `cmd/akvorado/inlet.go`.

use crate::ServiceArgs;
use crate::service::{Service, parse_config};
use flowscope_common::config::Validate;
use flowscope_common::{httpserver, reporter};
use flowscope_inlet::Inlet;
use flowscope_inlet::config::{FlowConfiguration, KafkaConfiguration};
use serde::{Deserialize, Serialize};

/// Configuration of the inlet service.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct InletConfiguration {
    pub reporting: reporter::Configuration,
    pub http: httpserver::Configuration,
    pub flow: FlowConfiguration,
    pub kafka: KafkaConfiguration,
}

impl Validate for InletConfiguration {
    fn validate(&self) -> Result<(), String> {
        self.http.validate()?;
        self.flow.validate()?;
        self.kafka.validate()
    }
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    let (config, _) = parse_config::<InletConfiguration>(&args, "inlet").await?;
    if config.flow.inputs.is_empty() {
        anyhow::bail!("no input configured");
    }
    if args.check {
        return Ok(());
    }
    let service = Service::new("inlet", &config.reporting, config.http.clone()).await?;
    let inlet = Inlet::start(&service.reporter, &config.flow, &config.kafka).map_err(anyhow::Error::msg)?;
    service.start().await?;
    service.daemon.spawn("inlet/flow", inlet.run(service.daemon.clone()));
    service.wait().await
}
