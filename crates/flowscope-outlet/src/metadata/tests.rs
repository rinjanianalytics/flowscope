// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use flowscope_common::config::{from_value, parse_str};

fn ip(s: &str) -> Ipv6Addr {
    flowscope_common::helpers::subnetmap::addr_to6(&s.parse().unwrap())
}

fn static_config() -> Configuration {
    from_value(
        parse_str(
            "providers:
  - type: static
    exporters:
      2001:db8:1::/48:
        name: nodefault
        if-indexes:
          10:
            name: Gi10
            description: 10th interface
            speed: 1000
      2001:db8:2::/48:
        name: default
        region: europe
        default:
          name: Default0
          description: Default interface
          speed: 1000
      2001:db8:3::/48:
        name: skip
        skip-missing-interfaces: true
",
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn static_lookups() {
    let r = Reporter::new_mock();
    let daemon = Daemon::new();
    let config = static_config();
    config.validate().unwrap();
    let m = Metadata::new(&r, config, &daemon).unwrap();
    let got = tokio::task::spawn_blocking(move || {
        let now = SystemTime::now();
        vec![
            m.lookup(now, ip("2001:db8:1::10"), 10),
            m.lookup(now, ip("2001:db8:1::10"), 11),
            m.lookup(now, ip("2001:db8:2::10"), 11),
            m.lookup(now, ip("2001:db8:3::10"), 11),
            m.lookup(now, ip("2001:db8:4::10"), 11),
        ]
    })
    .await
    .unwrap();
    assert_eq!(
        got[0],
        Answer {
            found: true,
            exporter: Exporter { name: "nodefault".into(), ..Default::default() },
            interface: Interface {
                name: "Gi10".into(),
                description: "10th interface".into(),
                speed: 1000,
                ..Default::default()
            },
        }
    );
    assert_eq!(got[1], Answer::default());
    assert!(got[2].found);
    assert_eq!(got[2].exporter.region, "europe");
    assert_eq!(got[2].interface.name, "Default0");
    assert_eq!(got[3], Answer::default());
    assert_eq!(got[4], Answer::default());
    let metrics = r.render_metrics();
    assert!(
        metrics.contains("akvorado_outlet_metadata_provider_skips_total{exporter=\"2001:db8:3::10\"} 1"),
        "{metrics}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn configuration() {
    let c: Configuration = from_value(parse_str("provider:\n  type: snmp\n  communities: private\n").unwrap()).unwrap();
    assert_eq!(c.providers.len(), 1);
    assert!(matches!(&c.providers[0], ProviderConfiguration::Snmp(_)));
    let dumped = flowscope_common::config::dump(&c).unwrap();
    let back: Configuration = from_value(parse_str(&dumped).unwrap()).unwrap();
    assert_eq!(back, c);
    let bad: Configuration = from_value(parse_str("cache-refresh: 1m\ncache-duration: 10m\n").unwrap()).unwrap();
    assert!(bad.validate().is_err());
}

#[test]
fn breaker() {
    let mut b = Breaker::new();
    for _ in 0..BREAKER_ERRORS - 1 {
        assert!(b.allow());
        b.failure();
    }
    assert!(b.allow());
    b.failure();
    assert!(!b.allow());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs snmpd"]
async fn snmp() {
    let r = Reporter::new_mock();
    let daemon = Daemon::new();
    let config: Configuration = from_value(
        parse_str("providers:\n  - type: snmp\n    ports: {127.0.0.1: 16161}\n    communities: [wrong, public]\n")
            .unwrap(),
    )
    .unwrap();
    let m = Metadata::new(&r, config, &daemon).unwrap();
    let got = tokio::task::spawn_blocking(move || m.lookup(SystemTime::now(), ip("127.0.0.1"), 1)).await.unwrap();
    assert_eq!(got.exporter.name, "test-router.example.com");
    assert_eq!(got.interface.name, "lo");
    assert!(got.found);
}
