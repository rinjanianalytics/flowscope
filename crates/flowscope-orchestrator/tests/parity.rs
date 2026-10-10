// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! The Rust migrations give the same database as the ones of Akvorado, and
//! each version accepts the database of the other without changes. Needs the
//! ClickHouse of `tests/interop` and an Akvorado binary, from one of its
//! releases, given in `FLOWSCOPE_TEST_AKVORADO_BIN`.

use flowscope_common::clickhousedb::{self, ClickHouseDb};
use flowscope_common::reporter::Reporter;
use flowscope_orchestrator::clickhouse::{ClickHouse, Configuration};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn server() -> String {
    std::env::var("FLOWSCOPE_TEST_CLICKHOUSE").unwrap_or_else(|_| "127.0.0.1:19000".into())
}

fn db(database: &str) -> Arc<ClickHouseDb> {
    ClickHouseDb::new(clickhousedb::Configuration {
        servers: vec![server()],
        database: database.into(),
        ..Default::default()
    })
    .unwrap()
}

fn exec(sql: &str) {
    db("default").with_conn(|c| c.execute(sql, &[])).unwrap();
}

fn rows(sql: &str) -> Vec<Vec<String>> {
    db("default")
        .with_conn(|c| c.query(sql, &[]))
        .unwrap()
        .iter_rows()
        .map(|r| {
            r.iter()
                .map(|v| match v {
                    flowscope_clickhouse::Value::String(s) => s.clone(),
                    flowscope_clickhouse::Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    other => format!("{other:?}"),
                })
                .collect()
        })
        .collect()
}

/// Everything about a database, with its name replaced by "DB".
fn snapshot(database: &str) -> BTreeMap<String, Vec<Vec<String>>> {
    let q = |sql: &str| {
        rows(&sql.replace("{db}", database))
            .into_iter()
            .map(|r| r.into_iter().map(|v| v.replace(database, "DB")).collect())
            .collect()
    };
    BTreeMap::from([
        (
            "tables".into(),
            q("SELECT name, engine, create_table_query, engine_full, sorting_key, primary_key, partition_key \
               FROM system.tables WHERE database = '{db}' ORDER BY name"),
        ),
        (
            "columns".into(),
            q("SELECT table, name, position, type, default_kind, default_expression, compression_codec, \
               is_in_sorting_key, is_in_primary_key FROM system.columns WHERE database = '{db}' ORDER BY table, position"),
        ),
        (
            "indices".into(),
            q("SELECT table, name, type_full, expr, granularity FROM system.data_skipping_indices \
               WHERE database = '{db}' ORDER BY table, name"),
        ),
    ])
}

/// Configuration of the orchestrator: ClickHouse and schema sections, as
/// YAML (without the database part).
#[derive(Clone, Default)]
struct Setup {
    clickhouse: String,
    schema: String,
}

/// Run the Rust migrations once. Returns the steps applied.
fn migrate_rust(database: &str, setup: &Setup) -> u64 {
    use flowscope_common::config::{from_value, parse_str};
    let clickhouse = format!("orchestrator-url: http://orchestrator:8080\n{}", setup.clickhouse);
    let config: Configuration = from_value(parse_str(&clickhouse).unwrap()).unwrap();
    let schema_config: flowscope_schema::Configuration =
        from_value(parse_str(if setup.schema.is_empty() { "{}" } else { &setup.schema }).unwrap()).unwrap();
    let schema = Arc::new(flowscope_schema::Component::new(schema_config).unwrap());
    let ch = ClickHouse::new(&Reporter::new_mock(), config, Some(db(database)), schema).unwrap();
    ch.migrate_now().unwrap();
    ch.migration_counts().0
}

fn indent(yaml: &str) -> String {
    yaml.lines().map(|l| format!("  {l}\n")).collect()
}

/// Run the Go orchestrator until its migrations are done. Returns the steps
/// applied.
fn migrate_go(database: &str, setup: &Setup) -> u64 {
    let dir = std::env::temp_dir().join(format!("flowscope-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join(format!("{database}.yaml"));
    std::fs::write(
        &config,
        format!(
            "automatic-restart: false\nhttp:\n  listen: 127.0.0.1:18081\nclickhousedb:\n  servers: [{}]\n  database: {database}\n\
             clickhouse:\n  orchestrator-url: http://orchestrator:8080\n{}kafka:\n  manage-topic: false\n{}",
            server(),
            indent(&setup.clickhouse),
            if setup.schema.is_empty() { String::new() } else { format!("schema:\n{}", indent(&setup.schema)) }
        ),
    )
    .unwrap();
    let binary = std::env::var("FLOWSCOPE_TEST_AKVORADO_BIN").expect("FLOWSCOPE_TEST_AKVORADO_BIN: an Akvorado binary");
    let mut child = Command::new(binary)
        .args(["orchestrator", config.to_str().unwrap()])
        .stdout(std::fs::File::create(dir.join(format!("{database}.out"))).unwrap())
        .stderr(std::fs::File::create(dir.join(format!("{database}.log"))).unwrap())
        .spawn()
        .expect("the Akvorado binary starts");
    let deadline = Instant::now() + Duration::from_secs(60);
    let metric = |name: &str, body: &str| -> Option<u64> {
        body.lines().find(|l| l.starts_with(name)).and_then(|l| l.rsplit_once(' ')).and_then(|(_, v)| v.parse().ok())
    };
    let applied = loop {
        if Instant::now() > deadline || child.try_wait().ok().flatten().is_some() {
            let _ = child.kill();
            let log = std::fs::read_to_string(dir.join(format!("{database}.log"))).unwrap_or_default()
                + &std::fs::read_to_string(dir.join(format!("{database}.out"))).unwrap_or_default();
            panic!("Go migrations not done:\n{}", log.lines().rev().take(15).collect::<Vec<_>>().join("\n"));
        }
        std::thread::sleep(Duration::from_millis(200));
        let Ok(body) = reqwest::blocking_get("http://127.0.0.1:18081/api/v0/orchestrator/metrics") else { continue };
        if metric("akvorado_orchestrator_clickhouse_running_migrations ", &body) == Some(0) {
            break metric("akvorado_orchestrator_clickhouse_migrations_applied_steps_total ", &body).unwrap_or(0);
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    applied
}

mod reqwest {
    /// A tiny blocking GET, to avoid a runtime in this test.
    pub fn blocking_get(url: &str) -> Result<String, String> {
        use std::io::{Read, Write};
        let rest = url.strip_prefix("http://").ok_or("http only")?;
        let (host, path) = rest.split_once('/').ok_or("no path")?;
        let mut s = std::net::TcpStream::connect(host).map_err(|e| e.to_string())?;
        write!(s, "GET /{path} HTTP/1.0\r\nHost: {host}\r\n\r\n").map_err(|e| e.to_string())?;
        let mut body = String::new();
        s.read_to_string(&mut body).map_err(|e| e.to_string())?;
        Ok(body.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
    }
}

#[test]
#[ignore = "needs ClickHouse and an Akvorado binary"]
fn same_database_as_go() {
    let id = std::process::id();
    let (go_db, rust_db) = (format!("parity_go_{id}"), format!("parity_rust_{id}"));
    for d in [&go_db, &rust_db] {
        exec(&format!("DROP DATABASE IF EXISTS {d}"));
        exec(&format!("CREATE DATABASE {d}"));
    }

    let setup = Setup::default();
    let go_applied = migrate_go(&go_db, &setup);
    let rust_applied = migrate_rust(&rust_db, &setup);
    eprintln!("applied: go={go_applied} rust={rust_applied}");
    assert_eq!(rust_applied, go_applied, "steps applied on an empty database");

    compare(&go_db, &rust_db);

    // Each version accepts the database of the other one.
    assert_eq!(migrate_rust(&rust_db, &setup), 0, "Rust on its own database");
    assert_eq!(migrate_rust(&go_db, &setup), 0, "Rust on the Go database");
    assert_eq!(migrate_go(&rust_db, &setup), 0, "Go on the Rust database");

    for d in [&go_db, &rust_db] {
        exec(&format!("DROP DATABASE {d}"));
    }
}

fn compare(go_db: &str, rust_db: &str) {
    let (go, rust) = (snapshot(go_db), snapshot(rust_db));
    eprintln!(
        "compared: {} tables, {} columns, {} indices",
        go["tables"].len(),
        go["columns"].len(),
        go["indices"].len()
    );
    for part in ["tables", "columns", "indices"] {
        let (g, r) = (&go[part], &rust[part]);
        assert_eq!(g.len(), r.len(), "{part}: number of rows");
        for (a, b) in g.iter().zip(r) {
            assert_eq!(a, b, "{part}");
        }
    }
}

/// Both versions upgrade a database created by Go the same way.
#[test]
#[ignore = "needs ClickHouse and an Akvorado binary"]
fn same_upgrade_as_go() {
    let id = std::process::id();
    let (go_db, rust_db) = (format!("upgrade_go_{id}"), format!("upgrade_rust_{id}"));
    for d in [&go_db, &rust_db] {
        exec(&format!("DROP DATABASE IF EXISTS {d}"));
        exec(&format!("CREATE DATABASE {d}"));
        migrate_go(d, &Setup::default());
    }
    let dict = std::env::temp_dir().join(format!("flowscope-parity-dict-{id}.csv"));
    std::fs::write(&dict, "addr,role\n2001:db8::1,core\n").unwrap();
    let setup = Setup {
        clickhouse: "resolutions:\n  - {interval: 0, ttl: 240h}\n  - {interval: 1m, ttl: 168h, table-settings: {min_bytes_for_wide_part: 1000}}\n  \
                     - {interval: 5m, ttl: 2160h}\n  - {interval: 1h, ttl: 8760h}\n"
            .into(),
        schema: format!(
            "enabled: [SrcVlan, DstVlan, SrcMAC, MPLSLabels]\n\
             custom-dictionaries:\n  ips:\n    layout: complex_key_hashed\n    source: {}\n    \
             keys: [{{name: addr, type: IPv6}}]\n    attributes: [{{name: role, type: String, default: unknown}}]\n    \
             dimensions: [SrcAddr, DstAddr]\n",
            dict.display()
        ),
    };
    let go_applied = migrate_go(&go_db, &setup);
    let rust_applied = migrate_rust(&rust_db, &setup);
    eprintln!("upgrade applied: go={go_applied} rust={rust_applied}");
    assert_eq!(rust_applied, go_applied, "steps applied for the upgrade");
    compare(&go_db, &rust_db);
    assert_eq!(migrate_rust(&go_db, &setup), 0, "Rust after the Go upgrade");
    assert_eq!(migrate_go(&rust_db, &setup), 0, "Go after the Rust upgrade");
    for d in [&go_db, &rust_db] {
        exec(&format!("DROP DATABASE {d}"));
    }
    let _ = std::fs::remove_file(dict);
}
