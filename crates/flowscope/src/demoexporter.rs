// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `demo-exporter` command. Port of `cmd/akvorado/demo-exporter.go`.

use crate::ServiceArgs;
use crate::service::{Service, parse_config};
use flowscope_common::config::Validate;
use flowscope_common::{httpserver, reporter};
use flowscope_demoexporter::bmp::Bmp;
use flowscope_demoexporter::config::{BmpConfiguration, FlowsConfiguration, SnmpConfiguration};
use flowscope_demoexporter::flows::Flows;
use flowscope_demoexporter::snmp::Snmp;
use serde::{Deserialize, Serialize};

/// Configuration of the demo exporter service.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct DemoExporterConfiguration {
    pub reporting: reporter::Configuration,
    pub http: httpserver::Configuration,
    pub snmp: SnmpConfiguration,
    pub bmp: BmpConfiguration,
    pub flows: FlowsConfiguration,
}

impl Validate for DemoExporterConfiguration {
    fn validate(&self) -> Result<(), String> {
        self.http.validate()?;
        self.snmp.validate()?;
        self.bmp.validate()?;
        self.flows.validate()
    }
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    let (config, _) = parse_config::<DemoExporterConfiguration>(&args, "demo-exporter").await?;
    if args.check {
        return Ok(());
    }
    let service = Service::new("demo-exporter", &config.reporting, config.http.clone()).await?;
    let snmp = Snmp::new(&service.reporter, &config.snmp).await.map_err(anyhow::Error::msg)?;
    let bmp = Bmp::new(&service.reporter, config.bmp.clone());
    let flows = Flows::new(&service.reporter, config.flows.clone());
    service.start().await?;
    let token = service.daemon.token();
    service.daemon.spawn("demo-exporter/snmp", snmp.run(token.clone()));
    if bmp.enabled() {
        service.daemon.spawn("demo-exporter/bmp", bmp.run(token.clone()));
    }
    service.daemon.spawn("demo-exporter/flows", flows.run(token));
    service.wait().await
}
