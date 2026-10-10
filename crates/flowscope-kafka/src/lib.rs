// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Kafka configuration shared by the inlet, the outlet and the
//! orchestrator. Port of `common/kafka`.

use flowscope_common::config::migrate;
use flowscope_common::helpers::{FLOWSCOPE_VERSION, TlsConfiguration, validate_listen};
use flowscope_common::migrated;
use rdkafka::ClientConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Kafka connection configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Topic name. The version of the message is appended.
    pub topic: String,
    /// Brokers to connect to.
    pub brokers: Vec<String>,
    /// TLS configuration.
    pub tls: TlsConfiguration,
    /// SASL configuration.
    pub sasl: SaslConfiguration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            topic: "flows".into(),
            brokers: vec!["127.0.0.1:9092".into()],
            tls: TlsConfiguration::default(),
            sasl: SaslConfiguration::default(),
        }
    }
}

/// `version` is deprecated. SASL settings used to live in the TLS section.
pub fn migrate_configuration(map: &mut Map<String, Value>) -> Result<(), String> {
    migrate::remove(map, "Version");
    let mut moved = Vec::new();
    if let Some(Value::Object(tls)) = migrate::get_mut(map, "TLS") {
        for (old, new) in [("SASLUsername", "username"), ("SASLPassword", "password"), ("SASLMechanism", "mechanism")] {
            if let Some(v) = migrate::remove(tls, old) {
                moved.push((new, v));
            }
        }
    }
    if !moved.is_empty() {
        let sasl = migrate::submap(map, "sasl");
        for (k, v) in moved {
            sasl.insert(k.to_string(), v);
        }
    }
    Ok(())
}

migrated!(Configuration, migrate_configuration);

/// SASL mechanism.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaslMechanism {
    #[default]
    None,
    Plain,
    #[serde(rename = "scram-sha256")]
    ScramSha256,
    #[serde(rename = "scram-sha512")]
    ScramSha512,
    Oauth,
}

/// SASL configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SaslConfiguration {
    /// Username (client ID for OAuth).
    pub username: String,
    /// Password (client secret for OAuth).
    pub password: String,
    /// Mechanism.
    pub mechanism: SaslMechanism,
    /// Token URL for OAuth.
    #[serde(rename = "oauth-token-url")]
    pub oauth_token_url: String,
    /// Scopes for OAuth.
    #[serde(rename = "oauth-scopes")]
    pub oauth_scopes: Vec<String>,
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.topic.is_empty() {
            return Err("kafka.topic is required".into());
        }
        if self.brokers.is_empty() {
            return Err("kafka.brokers needs at least one broker".into());
        }
        for b in &self.brokers {
            validate_listen(b).map_err(|e| format!("kafka.brokers: {e}"))?;
        }
        self.tls.validate()?;
        let s = &self.sasl;
        if s.mechanism != SaslMechanism::None && (s.username.is_empty() || s.password.is_empty()) {
            return Err("kafka.sasl.username and kafka.sasl.password are required with a SASL mechanism".into());
        }
        if !s.username.is_empty() && s.mechanism == SaslMechanism::None {
            return Err("kafka.sasl.mechanism is required with kafka.sasl.username".into());
        }
        match (s.mechanism == SaslMechanism::Oauth, s.oauth_token_url.is_empty()) {
            (true, true) => return Err("kafka.sasl.oauth-token-url is required with OAuth".into()),
            (false, false) => return Err("kafka.sasl.oauth-token-url is only for OAuth".into()),
            _ => {}
        }
        Ok(())
    }

    /// Build a librdkafka client configuration.
    pub fn client_config(&self) -> ClientConfig {
        let mut c = ClientConfig::new();
        c.set("bootstrap.servers", self.brokers.join(","));
        c.set("client.id", format!("flowscope-{FLOWSCOPE_VERSION}"));
        let sasl = self.sasl.mechanism != SaslMechanism::None;
        let protocol = match (self.tls.enable, sasl) {
            (false, false) => "plaintext",
            (true, false) => "ssl",
            (false, true) => "sasl_plaintext",
            (true, true) => "sasl_ssl",
        };
        c.set("security.protocol", protocol);
        if self.tls.enable {
            if !self.tls.ca_file.is_empty() {
                c.set("ssl.ca.location", &self.tls.ca_file);
            }
            if !self.tls.cert_file.is_empty() {
                c.set("ssl.certificate.location", &self.tls.cert_file);
                let key = if self.tls.key_file.is_empty() { &self.tls.cert_file } else { &self.tls.key_file };
                c.set("ssl.key.location", key);
            }
            if self.tls.skip_verify {
                c.set("enable.ssl.certificate.verification", "false");
                c.set("ssl.endpoint.identification.algorithm", "none");
            }
        }
        match self.sasl.mechanism {
            SaslMechanism::None => {}
            SaslMechanism::Plain | SaslMechanism::ScramSha256 | SaslMechanism::ScramSha512 => {
                let m = match self.sasl.mechanism {
                    SaslMechanism::Plain => "PLAIN",
                    SaslMechanism::ScramSha256 => "SCRAM-SHA-256",
                    _ => "SCRAM-SHA-512",
                };
                c.set("sasl.mechanism", m);
                c.set("sasl.username", &self.sasl.username);
                c.set("sasl.password", &self.sasl.password);
            }
            SaslMechanism::Oauth => {
                c.set("sasl.mechanism", "OAUTHBEARER");
                c.set("sasl.oauthbearer.method", "oidc");
                c.set("sasl.oauthbearer.client.id", &self.sasl.username);
                c.set("sasl.oauthbearer.client.secret", &self.sasl.password);
                c.set("sasl.oauthbearer.token.endpoint.url", &self.sasl.oauth_token_url);
                if !self.sasl.oauth_scopes.is_empty() {
                    c.set("sasl.oauthbearer.scope", self.sasl.oauth_scopes.join(" "));
                }
            }
        }
        c
    }
}

/// How records are spread over partitions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoadBalanceAlgorithm {
    /// Random partition.
    #[default]
    Random,
    /// Same exporter, same partition.
    ByExporter,
}

impl LoadBalanceAlgorithm {
    /// Key for a record.
    pub fn record_key(self, exporter: &str) -> Vec<u8> {
        match self {
            LoadBalanceAlgorithm::Random => rand::random::<u32>().to_be_bytes().to_vec(),
            LoadBalanceAlgorithm::ByExporter => exporter.as_bytes().to_vec(),
        }
    }
}

/// Compression codec for produced records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionCodec {
    None,
    Gzip,
    Snappy,
    #[default]
    Lz4,
    Zstd,
}

impl CompressionCodec {
    /// Name for librdkafka's "compression.codec".
    pub fn as_str(self) -> &'static str {
        match self {
            CompressionCodec::None => "none",
            CompressionCodec::Gzip => "gzip",
            CompressionCodec::Snappy => "snappy",
            CompressionCodec::Lz4 => "lz4",
            CompressionCodec::Zstd => "zstd",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    #[test]
    fn default_is_valid() {
        Configuration::default().validate().unwrap();
    }

    #[test]
    fn sasl_migration() {
        let got: Configuration = from_value(
            parse_str(
                "topic: flows\nversion: 2.8.1\ntls:\n  enable: true\n  sasl-username: hello\n  sasl-password: password\n  sasl-mechanism: plain\n",
            )
            .unwrap(),
        )
        .unwrap();
        let expected = Configuration {
            tls: TlsConfiguration { enable: true, ..Default::default() },
            sasl: SaslConfiguration {
                username: "hello".into(),
                password: "password".into(),
                mechanism: SaslMechanism::Plain,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(got, expected);
        got.validate().unwrap();
        let c = got.client_config();
        assert_eq!(c.get("security.protocol"), Some("sasl_ssl"));
        assert_eq!(c.get("sasl.mechanism"), Some("PLAIN"));
    }

    #[test]
    fn oauth() {
        let got: Configuration = from_value(
            parse_str(
                "sasl:\n  mechanism: oauth\n  username: id\n  password: secret\n  oauth-token-url: http://example.com/token\n  oauth-scopes: [a, b]\n",
            )
            .unwrap(),
        )
        .unwrap();
        got.validate().unwrap();
        let c = got.client_config();
        assert_eq!(c.get("security.protocol"), Some("sasl_plaintext"));
        assert_eq!(c.get("sasl.oauthbearer.scope"), Some("a b"));
    }

    #[test]
    fn codec() {
        for codec in ["none", "gzip", "snappy", "lz4", "zstd"] {
            let got: CompressionCodec = from_value(Value::String(codec.into())).unwrap();
            assert_eq!(got.as_str(), codec);
        }
        assert!(from_value::<CompressionCodec>(Value::String("unknown".into())).is_err());
    }

    #[test]
    fn record_key() {
        assert_eq!(LoadBalanceAlgorithm::ByExporter.record_key("192.0.2.1"), b"192.0.2.1");
        assert_eq!(LoadBalanceAlgorithm::Random.record_key("192.0.2.1").len(), 4);
    }
}
