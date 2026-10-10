// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! TLS client configuration. Port of `common/helpers/tls.go`.

use crate::config::migrate;
use crate::migrated;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// TLS configuration for a client.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct TlsConfiguration {
    /// Enable TLS.
    pub enable: bool,
    /// Do not check the remote certificate.
    pub skip_verify: bool,
    /// CA certificate file. Empty means the system CAs.
    pub ca_file: String,
    /// Client certificate file.
    pub cert_file: String,
    /// Client key file. Empty means it is in the certificate file.
    pub key_file: String,
}

/// `verify` was replaced by `skip-verify`.
fn migrate_tls(map: &mut Map<String, Value>) -> Result<(), String> {
    let verify = migrate::find_key(map, "Verify");
    let skip = migrate::find_key(map, "SkipVerify");
    if let (Some(v), Some(s)) = (&verify, &skip) {
        return Err(format!("cannot have both {v:?} and {s:?}"));
    }
    if let Some(v) = verify
        && let Some(Value::Bool(b)) = map.get(&v)
    {
        let b = *b;
        map.remove(&v);
        map.insert("skip-verify".into(), Value::Bool(!b));
    }
    Ok(())
}

migrated!(TlsConfiguration, migrate_tls);

impl TlsConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enable && (!self.ca_file.is_empty() || !self.cert_file.is_empty() || !self.key_file.is_empty()) {
            return Err("tls.enable is required when using CA, certificate or key files".into());
        }
        if !self.key_file.is_empty() && self.cert_file.is_empty() {
            return Err("tls.cert-file is required with tls.key-file".into());
        }
        Ok(())
    }
}

impl TlsConfiguration {
    /// Build a rustls client configuration. `None` when TLS is disabled.
    pub fn make_rustls_config(&self) -> Result<Option<std::sync::Arc<rustls::ClientConfig>>, String> {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        if !self.enable {
            return Ok(None);
        }
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?;
        let builder = if self.skip_verify {
            builder.dangerous().with_custom_certificate_verifier(std::sync::Arc::new(NoVerify(provider)))
        } else {
            let mut roots = rustls::RootCertStore::empty();
            if self.ca_file.is_empty() {
                for cert in rustls_native_certs::load_native_certs().certs {
                    let _ = roots.add(cert);
                }
            } else {
                let certs: Vec<CertificateDer> = CertificateDer::pem_file_iter(&self.ca_file)
                    .and_then(|i| i.collect())
                    .map_err(|e| format!("cannot read CA certificate: {e}"))?;
                if certs.is_empty() {
                    return Err("cannot parse CA certificate".into());
                }
                for cert in certs {
                    roots.add(cert).map_err(|e| format!("cannot parse CA certificate: {e}"))?;
                }
            }
            builder.with_root_certificates(roots)
        };
        let config = if self.cert_file.is_empty() {
            builder.with_no_client_auth()
        } else {
            let key_file = if self.key_file.is_empty() { &self.cert_file } else { &self.key_file };
            let certs: Vec<CertificateDer> = CertificateDer::pem_file_iter(&self.cert_file)
                .and_then(|i| i.collect())
                .map_err(|e| format!("cannot read user certificate: {e}"))?;
            let key = PrivateKeyDer::from_pem_file(key_file).map_err(|e| format!("cannot read user key: {e}"))?;
            builder.with_client_auth_cert(certs, key).map_err(|e| format!("cannot read user certificate: {e}"))?
        };
        Ok(Some(std::sync::Arc::new(config)))
    }
}

/// Certificate verifier accepting anything (`skip-verify`). Signatures are
/// still checked so the handshake stays consistent.
#[derive(Debug)]
struct NoVerify(std::sync::Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{from_value, parse_str};

    #[test]
    fn verify_migration() {
        let got: TlsConfiguration = from_value(parse_str("enable: true\nverify: false\n").unwrap()).unwrap();
        assert_eq!(got, TlsConfiguration { enable: true, skip_verify: true, ..Default::default() });
        let got: TlsConfiguration = from_value(parse_str("Enable: true\nSkipVerify: true\n").unwrap()).unwrap();
        assert!(got.skip_verify);
        assert!(from_value::<TlsConfiguration>(parse_str("verify: false\nskip-verify: true\n").unwrap()).is_err());
    }

    #[test]
    fn rustls_config() {
        assert!(TlsConfiguration::default().make_rustls_config().unwrap().is_none());
        let c = TlsConfiguration { enable: true, skip_verify: true, ..Default::default() };
        assert!(c.make_rustls_config().unwrap().is_some());
        let c = TlsConfiguration { enable: true, ..Default::default() };
        assert!(c.make_rustls_config().unwrap().is_some());
        let c = TlsConfiguration { enable: true, ca_file: "/nonexistent".into(), ..Default::default() };
        assert!(c.make_rustls_config().is_err());
    }
}
