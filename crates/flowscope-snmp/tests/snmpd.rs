// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Tests against net-snmp. Needs
//! `docker compose -f tests/interop/docker-compose.yml up -d snmpd`.

use flowscope_snmp::ber::parse_oid;
use flowscope_snmp::{AuthProtocol, Credentials, Error, Options, PrivProtocol, Value, get_v2c, get_v3};
use std::time::Duration;

fn target() -> std::net::SocketAddr {
    std::env::var("FLOWSCOPE_TEST_SNMPD").unwrap_or_else(|_| "127.0.0.1:16161".into()).parse().unwrap()
}

fn options() -> Options {
    Options { timeout: Duration::from_secs(2), retries: 1 }
}

fn oids() -> Vec<Vec<u32>> {
    vec![
        parse_oid("1.3.6.1.2.1.1.5.0").unwrap(),
        parse_oid("1.3.6.1.2.1.2.2.1.2.1").unwrap(),
        parse_oid("1.3.6.1.2.1.31.1.1.1.18.999").unwrap(),
    ]
}

fn check(r: &flowscope_snmp::Response) {
    assert_eq!(r.error_status, 0);
    assert_eq!(r.varbinds[0].1, Value::OctetString(b"test-router.example.com".to_vec()));
    assert_eq!(r.varbinds[1].1, Value::OctetString(b"lo".to_vec()));
    assert_eq!(r.varbinds[2].1, Value::NoSuchInstance);
}

#[tokio::test]
#[ignore = "needs snmpd"]
async fn v2c() {
    let r = get_v2c(target(), "public", &oids(), &options(), &mut || {}).await.unwrap();
    check(&r);
    let err =
        get_v2c(target(), "wrong", &oids(), &Options { timeout: Duration::from_millis(300), retries: 0 }, &mut || {})
            .await
            .unwrap_err();
    assert_eq!(err, Error::Timeout);
}

#[tokio::test]
#[ignore = "needs snmpd"]
async fn v3() {
    let user = |name: &str, auth, privacy| Credentials::User {
        user_name: name.into(),
        auth,
        auth_passphrase: "authpassword".into(),
        privacy,
        priv_passphrase: "privpassword".into(),
        context_name: String::new(),
    };
    for creds in [
        user("noauth", AuthProtocol::None, PrivProtocol::None),
        user("md5des", AuthProtocol::Md5, PrivProtocol::Des),
        user("shaaes", AuthProtocol::Sha, PrivProtocol::Aes),
        user("sha256aes", AuthProtocol::Sha256, PrivProtocol::Aes),
        user("sha512", AuthProtocol::Sha512, PrivProtocol::None),
    ] {
        let mut state = None;
        let r = get_v3(target(), &creds, &oids(), &options(), &mut state, &mut || {}).await;
        check(&r.unwrap_or_else(|e| panic!("{creds:?}: {e}")));
        // Second request uses the cached engine state.
        assert!(state.is_some());
        let r = get_v3(target(), &creds, &oids(), &options(), &mut state, &mut || {}).await.unwrap();
        check(&r);
    }
    // Wrong password.
    let mut state = None;
    let creds = Credentials::User {
        user_name: "shaaes".into(),
        auth: AuthProtocol::Sha,
        auth_passphrase: "wrongpassword".into(),
        privacy: PrivProtocol::Aes,
        priv_passphrase: "privpassword".into(),
        context_name: String::new(),
    };
    assert!(get_v3(target(), &creds, &oids(), &options(), &mut state, &mut || {}).await.is_err());
}
