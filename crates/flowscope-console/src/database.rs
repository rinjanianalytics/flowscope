// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Database for the console settings (saved filters). The table and the
//! queries are the ones of the Go version, so both can share a database.
//! The DSN is written like for Go drivers.

use flowscope_common::config::Validate;
use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlSslMode};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

/// Configuration of the database.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Driver: sqlite, postgresql or mysql.
    pub driver: String,
    /// DSN, as for the Go drivers.
    #[serde(rename = "dsn")]
    pub dsn: String,
    /// Saved filters available to all users.
    pub saved_filters: Vec<BuiltinSavedFilter>,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration { driver: "sqlite".into(), dsn: "file::memory:?cache=shared".into(), saved_filters: vec![] }
    }
}

impl Validate for Configuration {
    fn validate(&self) -> Result<(), String> {
        if self.driver.is_empty() {
            return Err("Driver is required".into());
        }
        if self.dsn.is_empty() {
            return Err("DSN is required".into());
        }
        for f in &self.saved_filters {
            if f.description.is_empty() || f.content.is_empty() {
                return Err("SavedFilters need a description and a content".into());
            }
        }
        Ok(())
    }
}

/// A saved filter available to all users.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuiltinSavedFilter {
    pub description: String,
    pub content: String,
}

/// A saved filter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedFilter {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub content: String,
}

/// User owning the builtin filters.
const SYSTEM_USER: &str = "__system";

#[derive(Clone, Debug)]
enum Pool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
    MySql(MySqlPool),
}

/// The database component.
#[derive(Clone, Debug)]
pub struct Database {
    config: Configuration,
    pool: Pool,
}

type Row = (i64, Option<String>, Option<bool>, Option<String>, Option<String>);

fn to_filter((id, user, shared, description, content): Row) -> SavedFilter {
    SavedFilter {
        id: id as u64,
        user: user.unwrap_or_default(),
        shared: shared.unwrap_or_default(),
        description: description.unwrap_or_default(),
        content: content.unwrap_or_default(),
    }
}

/// Run a query on the pool, with the SQL written for SQLite: `?`
/// placeholders and identifiers between double quotes. They are rewritten
/// for the other databases.
macro_rules! on_pool {
    ($pool:expr, $sql:expr, |$q:ident| $body:expr) => {{
        let sql: &str = $sql;
        match $pool {
            Pool::Sqlite(pool) => {
                let $q = sqlx::query_as::<_, Row>(sql);
                let $q = $body;
                $q.fetch_all(pool).await.map(|rows| rows.into_iter().map(to_filter).collect::<Vec<_>>())
            }
            Pool::Postgres(pool) => {
                let sql = postgres_sql(sql);
                let $q = sqlx::query_as::<_, Row>(&sql);
                let $q = $body;
                $q.fetch_all(pool).await.map(|rows| rows.into_iter().map(to_filter).collect::<Vec<_>>())
            }
            Pool::MySql(pool) => {
                let sql = sql.replace('"', "`");
                let $q = sqlx::query_as::<_, Row>(&sql);
                let $q = $body;
                $q.fetch_all(pool).await.map(|rows| rows.into_iter().map(to_filter).collect::<Vec<_>>())
            }
        }
    }};
}

/// Same, for a statement without result. Returns the affected rows.
macro_rules! exec_on_pool {
    ($pool:expr, $sql:expr, |$q:ident| $body:expr) => {{
        let sql: &str = $sql;
        match $pool {
            Pool::Sqlite(pool) => {
                let $q = sqlx::query(sql);
                let $q = $body;
                $q.execute(pool).await.map(|r| r.rows_affected())
            }
            Pool::Postgres(pool) => {
                let sql = postgres_sql(sql);
                let $q = sqlx::query(&sql);
                let $q = $body;
                $q.execute(pool).await.map(|r| r.rows_affected())
            }
            Pool::MySql(pool) => {
                let sql = sql.replace('"', "`");
                let $q = sqlx::query(&sql);
                let $q = $body;
                $q.execute(pool).await.map(|r| r.rows_affected())
            }
        }
    }};
}

/// Number the placeholders for PostgreSQL.
fn postgres_sql(sql: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in sql.chars() {
        if c == '?' {
            n += 1;
            out.push_str(&format!("${n}"));
        } else {
            out.push(c);
        }
    }
    out
}

const COLUMNS: &str = r#""id", "user", "shared", "description", "content""#;

impl Database {
    /// Connect to the database, create the table if needed and add the
    /// builtin filters.
    pub async fn new(config: Configuration) -> Result<Database, String> {
        let pool = match config.driver.as_str() {
            "sqlite" => {
                let (options, memory) = sqlite_options(&config.dsn)?;
                let mut pool = SqlitePoolOptions::new();
                if memory {
                    // Each connection to an in-memory database gets its own
                    // database: keep a single one, forever.
                    pool = pool.max_connections(1).min_connections(1).idle_timeout(None).max_lifetime(None);
                }
                Pool::Sqlite(
                    pool.connect_with(options).await.map_err(|e| format!("unable to open sqlite database: {e}"))?,
                )
            }
            "postgresql" => Pool::Postgres(
                PgPoolOptions::new()
                    .connect_with(postgres_options(&config.dsn)?)
                    .await
                    .map_err(|e| format!("unable to open PostgreSQL database: {e}"))?,
            ),
            "mysql" => Pool::MySql(
                MySqlPoolOptions::new()
                    .connect_with(mysql_options(&config.dsn)?)
                    .await
                    .map_err(|e| format!("unable to open MySQL database: {e}"))?,
            ),
            driver => return Err(format!("{driver:?} is not a supporter driver")),
        };
        let db = Database { config, pool };
        db.migrate().await.map_err(|e| format!("cannot migrate database: {e}"))?;
        db.populate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        let create = match self.pool {
            Pool::Sqlite(_) => {
                r#"CREATE TABLE IF NOT EXISTS "saved_filters" ("id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, "user" VARCHAR, "shared" BOOLEAN, "description" VARCHAR, "content" VARCHAR)"#
            }
            Pool::Postgres(_) => {
                r#"CREATE TABLE IF NOT EXISTS "saved_filters" ("id" BIGSERIAL NOT NULL, "user" VARCHAR, "shared" BOOLEAN, "description" VARCHAR, "content" VARCHAR, PRIMARY KEY ("id"))"#
            }
            Pool::MySql(_) => {
                r#"CREATE TABLE IF NOT EXISTS "saved_filters" ("id" BIGINT NOT NULL AUTO_INCREMENT, "user" VARCHAR(255), "shared" BOOLEAN, "description" VARCHAR(255), "content" VARCHAR(255), PRIMARY KEY ("id"))"#
            }
        };
        exec_on_pool!(&self.pool, create, |q| q)?;
        let index = match self.pool {
            // MySQL has no IF NOT EXISTS for indexes.
            Pool::MySql(_) => r#"CREATE INDEX "idx_saved_filters_user" ON "saved_filters" ("user")"#,
            _ => r#"CREATE INDEX IF NOT EXISTS "idx_saved_filters_user" ON "saved_filters" ("user")"#,
        };
        match exec_on_pool!(&self.pool, index, |q| q) {
            // Duplicate key name: the index already exists.
            Err(sqlx::Error::Database(e))
                if e.code().as_deref() == Some("42000") && e.message().contains("Duplicate") =>
            {
                Ok(())
            }
            r => r.map(|_| ()),
        }
    }

    /// Add the builtin filters, and remove the old ones.
    async fn populate(&self) -> Result<(), String> {
        for filter in &self.config.saved_filters {
            tracing::debug!(description = %filter.description, "add builtin filter");
            let existing = on_pool!(
                &self.pool,
                &format!(
                    r#"SELECT {COLUMNS} FROM "saved_filters" WHERE ("user" = ?) AND ("description" = ?) AND ("content" = ?) LIMIT 1"#
                ),
                |q| q.bind(SYSTEM_USER).bind(&filter.description).bind(&filter.content)
            )
            .map_err(|e| format!("cannot lookup builtin filter: {e}"))?;
            if existing.is_empty() {
                exec_on_pool!(
                    &self.pool,
                    r#"INSERT INTO "saved_filters" ("user", "shared", "description", "content") VALUES (?, ?, ?, ?)"#,
                    |q| q.bind(SYSTEM_USER).bind(true).bind(&filter.description).bind(&filter.content)
                )
                .map_err(|e| format!("unable add builtin filter: {e}"))?;
            }
        }
        let results = on_pool!(
            &self.pool,
            &format!(r#"SELECT {COLUMNS} FROM "saved_filters" WHERE ("user" = ?) AND ("shared" = ?)"#),
            |q| q.bind(SYSTEM_USER).bind(true)
        )
        .map_err(|e| format!("cannot get existing builtin filters: {e}"))?;
        for result in results {
            if self
                .config
                .saved_filters
                .iter()
                .any(|f| f.description == result.description && f.content == result.content)
            {
                continue;
            }
            tracing::info!(description = %result.description, "remove old builtin filter");
            exec_on_pool!(&self.pool, r#"DELETE FROM "saved_filters" WHERE ("id" = ?)"#, |q| q.bind(result.id as i64))
                .map_err(|e| format!("cannot delete old builtin filter: {e}"))?;
        }
        Ok(())
    }

    /// Create a saved filter.
    pub async fn create_saved_filter(&self, f: &SavedFilter) -> Result<(), String> {
        exec_on_pool!(
            &self.pool,
            r#"INSERT INTO "saved_filters" ("user", "shared", "description", "content") VALUES (?, ?, ?, ?)"#,
            |q| q.bind(&f.user).bind(f.shared).bind(&f.description).bind(&f.content)
        )
        .map(|_| ())
        .map_err(|e| format!("unable to create new saved filter: {e}"))
    }

    /// The saved filters of a user, and the shared ones.
    pub async fn list_saved_filters(&self, user: &str) -> Result<Vec<SavedFilter>, String> {
        on_pool!(
            &self.pool,
            &format!(r#"SELECT {COLUMNS} FROM "saved_filters" WHERE ("user" = ?) OR ("shared" = ?)"#),
            |q| q.bind(user).bind(true)
        )
        .map_err(|e| format!("unable to retrieve saved filters: {e}"))
    }

    /// Delete a saved filter. When a user is given, the filter has to
    /// belong to them.
    pub async fn delete_saved_filter(&self, id: u64, user: &str) -> Result<(), String> {
        let rows = if user.is_empty() {
            exec_on_pool!(&self.pool, r#"DELETE FROM "saved_filters" WHERE ("id" = ?)"#, |q| q.bind(id as i64))
        } else {
            exec_on_pool!(&self.pool, r#"DELETE FROM "saved_filters" WHERE ("id" = ?) AND ("user" = ?)"#, |q| q
                .bind(id as i64)
                .bind(user))
        }
        .map_err(|e| format!("cannot delete saved filter: {e}"))?;
        if rows == 0 {
            return Err("no matching saved filter to delete".into());
        }
        Ok(())
    }
}

/// SQLite options from a DSN of the Go driver (`file:path?params`, a path or
/// `:memory:`). Also tells if the database is in memory.
fn sqlite_options(dsn: &str) -> Result<(SqliteConnectOptions, bool), String> {
    let path = dsn.strip_prefix("file:").unwrap_or(dsn);
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let memory = path == ":memory:" || path.is_empty() || query.split('&').any(|p| p == "mode=memory");
    if memory {
        return Ok((SqliteConnectOptions::from_str("sqlite::memory:").map_err(|e| e.to_string())?, true));
    }
    let mut options = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    for param in query.split('&').filter(|p| !p.is_empty()) {
        match param.split_once('=') {
            Some(("mode", "ro")) => options = options.read_only(true),
            Some(("_pragma", pragma)) => {
                let pragma = pragma.replace("%20", " ");
                if let Some((name, value)) = pragma.split_once('(').and_then(|(n, v)| Some((n, v.strip_suffix(')')?))) {
                    options = options.pragma(name.to_string(), value.to_string());
                }
            }
            _ => {}
        }
    }
    Ok((options, false))
}

/// PostgreSQL options from a URL or a "key=value" DSN.
fn postgres_options(dsn: &str) -> Result<PgConnectOptions, String> {
    if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
        return PgConnectOptions::from_str(dsn).map_err(|e| format!("invalid PostgreSQL DSN: {e}"));
    }
    let mut options = PgConnectOptions::new();
    for pair in dsn.split_whitespace() {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("invalid PostgreSQL DSN element {pair:?}"));
        };
        let value = value.trim_matches('\'');
        options = match key {
            "host" => options.host(value),
            "port" => options.port(value.parse().map_err(|_| format!("invalid port {value:?}"))?),
            "user" => options.username(value),
            "password" => options.password(value),
            "dbname" => options.database(value),
            "sslmode" => options.ssl_mode(value.parse().map_err(|_| format!("invalid sslmode {value:?}"))?),
            "application_name" => options.application_name(value),
            _ => options,
        };
    }
    Ok(options)
}

/// MySQL options from a DSN of the Go driver:
/// `[user[:password]@][net[(addr)]]/dbname[?params]`.
fn mysql_options(dsn: &str) -> Result<MySqlConnectOptions, String> {
    if dsn.starts_with("mysql://") {
        return MySqlConnectOptions::from_str(dsn).map_err(|e| format!("invalid MySQL DSN: {e}"));
    }
    let (rest, params) = dsn.split_once('?').unwrap_or((dsn, ""));
    let slash = rest.rfind('/').ok_or("invalid MySQL DSN: missing the slash before the database name")?;
    let (head, database) = (&rest[..slash], &rest[slash + 1..]);
    let (credentials, address) = match head.rfind('@') {
        Some(at) => (&head[..at], &head[at + 1..]),
        None => ("", head),
    };
    let mut options = MySqlConnectOptions::new();
    if !credentials.is_empty() {
        let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
        options = options.username(user);
        if !password.is_empty() {
            options = options.password(password);
        }
    }
    if !database.is_empty() {
        options = options.database(database);
    }
    if let Some(open) = address.find('(') {
        let net = &address[..open];
        let addr = address[open + 1..].trim_end_matches(')');
        match net {
            "unix" => options = options.socket(addr),
            _ => {
                let (host, port) = match addr.rsplit_once(':') {
                    Some((h, p)) => (h, p.parse().map_err(|_| format!("invalid port {p:?}"))?),
                    None => (addr, 3306),
                };
                options = options.host(host.trim_matches(|c| c == '[' || c == ']')).port(port);
            }
        }
    } else if address == "unix" {
        options = options.socket("/tmp/mysql.sock");
    }
    for param in params.split('&').filter(|p| !p.is_empty()) {
        if let Some((key, value)) = param.split_once('=') {
            match key {
                "tls" => {
                    options = options.ssl_mode(match value {
                        "true" => MySqlSslMode::VerifyIdentity,
                        "skip-verify" | "preferred" => MySqlSslMode::Preferred,
                        _ => MySqlSslMode::Disabled,
                    })
                }
                "charset" => options = options.charset(value.split(',').next().unwrap_or(value)),
                "collation" => options = options.collation(value),
                _ => {}
            }
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory(saved: Vec<BuiltinSavedFilter>) -> Database {
        Database::new(Configuration { saved_filters: saved, ..Default::default() }).await.unwrap()
    }

    #[tokio::test]
    async fn saved_filters() {
        let db = memory(vec![]).await;
        assert_eq!(db.list_saved_filters("marty").await.unwrap(), vec![]);
        let f = |user: &str, shared, description: &str| SavedFilter {
            user: user.into(),
            shared,
            description: description.into(),
            content: "InIfBoundary = external".into(),
            ..Default::default()
        };
        db.create_saved_filter(&f("marty", false, "marty's")).await.unwrap();
        db.create_saved_filter(&f("judith", true, "judith's")).await.unwrap();
        db.create_saved_filter(&f("judith", false, "secret")).await.unwrap();
        let got = db.list_saved_filters("marty").await.unwrap();
        assert_eq!(
            got.iter().map(|f| (f.id, f.description.as_str())).collect::<Vec<_>>(),
            vec![(1, "marty's"), (2, "judith's")]
        );
        assert!(db.delete_saved_filter(2, "marty").await.is_err());
        db.delete_saved_filter(1, "marty").await.unwrap();
        assert!(db.delete_saved_filter(1, "marty").await.is_err());
        assert_eq!(db.list_saved_filters("marty").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn builtin_filters() {
        let builtin = |d: &str| BuiltinSavedFilter { description: d.into(), content: "SrcAS = 1".into() };
        let path = std::env::temp_dir().join(format!("akvorado-test-{}.sqlite", std::process::id()));
        let dsn = format!("file:{}", path.display());
        let config = |saved| Configuration { driver: "sqlite".into(), dsn: dsn.clone(), saved_filters: saved };
        let db = Database::new(config(vec![builtin("first"), builtin("second")])).await.unwrap();
        let names = |v: Vec<SavedFilter>| v.into_iter().map(|f| f.description).collect::<Vec<_>>();
        assert_eq!(names(db.list_saved_filters("someone").await.unwrap()), vec!["first", "second"]);
        drop(db);
        // Restarting keeps the builtin filters once, removes the old ones.
        let db = Database::new(config(vec![builtin("second"), builtin("third")])).await.unwrap();
        assert_eq!(names(db.list_saved_filters("someone").await.unwrap()), vec!["second", "third"]);
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dsn() {
        let o =
            mysql_options("akvorado:secret@tcp(db.example.com:3307)/akvorado?charset=utf8mb4&parseTime=true").unwrap();
        assert_eq!(o.get_host(), "db.example.com");
        assert_eq!(o.get_port(), 3307);
        assert_eq!(o.get_username(), "akvorado");
        assert_eq!(o.get_database(), Some("akvorado"));
        let o = postgres_options("host=db port=5433 user=akvorado password=x dbname=console sslmode=disable").unwrap();
        assert_eq!(o.get_host(), "db");
        assert_eq!(o.get_port(), 5433);
        assert_eq!(o.get_database(), Some("console"));
        assert!(sqlite_options("file::memory:?cache=shared").unwrap().1);
        assert!(!sqlite_options("file:/var/lib/akvorado/console.sqlite").unwrap().1);
    }
}
