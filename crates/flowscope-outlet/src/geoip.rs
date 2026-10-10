// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! GeoIP databases (MaxMind or IPinfo format). They are not queried for each
//! flow: the networks component walks them to build its prefix tree. Port of
//! `outlet/geoip`.

use flowscope_common::config::migrate;
use flowscope_common::daemon::Daemon;
use flowscope_common::migrated;
use flowscope_common::reporter::{CounterVec, Reporter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

/// GeoIP configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    #[serde(rename = "asn-database")]
    pub asn_database: Vec<String>,
    pub geo_database: Vec<String>,
    /// Do not fail when a database cannot be opened.
    pub optional: bool,
}

fn migrate_geoip(map: &mut Map<String, Value>) -> Result<(), String> {
    migrate::rename(map, "CountryDatabase", "geo-database")?;
    // A single database is accepted.
    for k in ["ASNDatabase", "GeoDatabase"] {
        if let Some(v) = migrate::get_mut(map, k)
            && v.is_string()
        {
            *v = Value::Array(vec![v.clone()]);
        }
    }
    Ok(())
}

migrated!(Configuration, migrate_geoip);

/// Geographic information for a prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeoInfo {
    pub country: String,
    pub city: String,
    pub state: String,
}

#[derive(Deserialize)]
struct Names {
    en: Option<String>,
}

#[derive(Deserialize)]
struct MaxmindCountry {
    iso_code: Option<String>,
}

#[derive(Deserialize)]
struct MaxmindCity {
    names: Option<Names>,
}

#[derive(Deserialize)]
struct MaxmindSubdivision {
    iso_code: Option<String>,
    names: Option<Names>,
}

#[derive(Deserialize)]
struct MaxmindGeo {
    country: Option<MaxmindCountry>,
    city: Option<MaxmindCity>,
    subdivisions: Option<Vec<MaxmindSubdivision>>,
}

/// An unsigned integer of any width (the MaxMind decoder is strict about
/// widths).
struct AnyUint(u64);

impl<'de> Deserialize<'de> for AnyUint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = AnyUint;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an unsigned integer")
            }
            fn visit_u64<E>(self, v: u64) -> Result<AnyUint, E> {
                Ok(AnyUint(v))
            }
            fn visit_u32<E>(self, v: u32) -> Result<AnyUint, E> {
                Ok(AnyUint(v as u64))
            }
            fn visit_u16<E>(self, v: u16) -> Result<AnyUint, E> {
                Ok(AnyUint(v as u64))
            }
            fn visit_u8<E>(self, v: u8) -> Result<AnyUint, E> {
                Ok(AnyUint(v as u64))
            }
            fn visit_u128<E: serde::de::Error>(self, v: u128) -> Result<AnyUint, E> {
                u64::try_from(v).map(AnyUint).map_err(|_| E::custom("integer too large"))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct MaxmindAsn {
    autonomous_system_number: Option<AnyUint>,
}

#[derive(Deserialize)]
struct IpinfoGeo {
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
}

#[derive(Deserialize)]
struct IpinfoAsn {
    asn: Option<String>,
}

struct Database {
    reader: maxminddb::Reader<maxminddb::Mmap>,
    ipinfo: bool,
}

impl Database {
    fn open(path: &str) -> Result<Database, String> {
        // SAFETY: the file is memory-mapped read-only; it is replaced
        // atomically by the updaters, not modified in place.
        let reader = unsafe { maxminddb::Reader::open_mmap(path) }.map_err(|e| e.to_string())?;
        let ipinfo = reader.metadata().database_type.starts_with("ipinfo ");
        Ok(Database { reader, ipinfo })
    }

    fn for_each<T: for<'de> Deserialize<'de>>(&self, mut f: impl FnMut(&str, T)) {
        let Ok(iter) = self.reader.networks(Default::default()) else { return };
        for item in iter.flatten() {
            let Ok(network) = item.network() else { continue };
            if let Ok(Some(v)) = item.decode::<T>() {
                f(&network.to_string(), v);
            }
        }
    }

    fn iter_geo(&self, f: &mut dyn FnMut(&str, GeoInfo)) {
        if self.ipinfo {
            self.for_each::<IpinfoGeo>(|p, g| {
                let info = GeoInfo {
                    country: g.country.unwrap_or_default(),
                    state: g.region.unwrap_or_default(),
                    city: g.city.unwrap_or_default(),
                };
                if info != GeoInfo::default() {
                    f(p, info);
                }
            });
        } else {
            self.for_each::<MaxmindGeo>(|p, g| {
                let state = g
                    .subdivisions
                    .and_then(|s| s.into_iter().next())
                    .map(|s| s.iso_code.or(s.names.and_then(|n| n.en)).unwrap_or_default())
                    .unwrap_or_default();
                let info = GeoInfo {
                    country: g.country.and_then(|c| c.iso_code).unwrap_or_default(),
                    city: g.city.and_then(|c| c.names).and_then(|n| n.en).unwrap_or_default(),
                    state,
                };
                if info != GeoInfo::default() {
                    f(p, info);
                }
            });
        }
    }

    fn iter_asn(&self, f: &mut dyn FnMut(&str, u32)) {
        if self.ipinfo {
            self.for_each::<IpinfoAsn>(|p, a| {
                if let Some(asn) = a.asn.and_then(|s| s.strip_prefix("AS").and_then(|d| d.parse::<u32>().ok()))
                    && asn != 0
                {
                    f(p, asn);
                }
            });
        } else {
            self.for_each::<MaxmindAsn>(|p, a| {
                if let Some(asn) = a.autonomous_system_number.and_then(|v| u32::try_from(v.0).ok())
                    && asn != 0
                {
                    f(p, asn);
                }
            });
        }
    }
}

/// The GeoIP component.
pub struct GeoIp {
    config: Configuration,
    geo: RwLock<Vec<Option<Arc<Database>>>>,
    asn: RwLock<Vec<Option<Arc<Database>>>>,
    subscribers: Mutex<Vec<tokio::sync::mpsc::Sender<()>>>,
    refresh: CounterVec,
}

fn clean_paths(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let clean = PathBuf::from(p).components().collect::<PathBuf>().to_string_lossy().to_string();
        out.retain(|x| *x != clean);
        out.push(clean);
    }
    out
}

impl GeoIp {
    pub fn new(r: &Reporter, mut config: Configuration) -> Arc<GeoIp> {
        config.geo_database = clean_paths(&config.geo_database);
        config.asn_database = clean_paths(&config.asn_database);
        Arc::new(GeoIp {
            geo: RwLock::new(vec![None; config.geo_database.len()]),
            asn: RwLock::new(vec![None; config.asn_database.len()]),
            subscribers: Mutex::new(Vec::new()),
            refresh: r.metrics("outlet/geoip").counter_vec(
                "db_refresh_total",
                "Refresh event for a GeoIP database.",
                &["database"],
            ),
            config,
        })
    }

    /// Get notified when a database is reloaded.
    pub fn notify(&self) -> tokio::sync::mpsc::Receiver<()> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    fn open(&self, which: &str, path: &str) -> Result<(), String> {
        let db = Database::open(path).map_err(|e| {
            tracing::error!(database = path, error = %e, "cannot open {which} database");
            format!("cannot open {which} database: {e}")
        })?;
        let (configured, slots) = if which == "asn" {
            (&self.config.asn_database, &self.asn)
        } else {
            (&self.config.geo_database, &self.geo)
        };
        let index =
            configured.iter().position(|p| p == path).ok_or_else(|| format!("unknown {which} database {path:?}"))?;
        slots.write().unwrap()[index] = Some(Arc::new(db));
        self.refresh.with_label_values(&[which]).inc();
        for s in self.subscribers.lock().unwrap().iter() {
            let _ = s.try_send(());
        }
        Ok(())
    }

    /// Open the databases and watch them for changes.
    pub fn start(self: &Arc<Self>, daemon: &Daemon) -> Result<(), String> {
        if self.config.geo_database.is_empty() && self.config.asn_database.is_empty() {
            tracing::warn!("skipping GeoIP component: no database specified");
            return Ok(());
        }
        tracing::info!("starting GeoIP component");
        for p in self.config.geo_database.clone() {
            if let Err(e) = self.open("geo", &p)
                && !self.config.optional
            {
                return Err(e);
            }
        }
        for p in self.config.asn_database.clone() {
            if let Err(e) = self.open("asn", &p)
                && !self.config.optional
            {
                return Err(e);
            }
        }
        // Watch the files by polling their modification time.
        let this = self.clone();
        let token = daemon.token();
        daemon.spawn_background(async move {
            let mtime = |p: &str| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let mut known: BTreeMap<String, Option<SystemTime>> = this
                .config
                .geo_database
                .iter()
                .chain(&this.config.asn_database)
                .map(|p| (p.clone(), mtime(p)))
                .collect();
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                for (path, last) in known.iter_mut() {
                    let now = mtime(path);
                    if now.is_some() && now != *last {
                        *last = now;
                        tracing::debug!(database = %path, "GeoIP database changed");
                        let which = if this.config.asn_database.contains(path) { "asn" } else { "geo" };
                        let _ = this.open(which, path);
                    }
                }
            }
        });
        Ok(())
    }

    /// Walk the geo databases.
    pub fn iter_geo(&self, f: &mut dyn FnMut(&str, GeoInfo)) {
        for db in self.geo.read().unwrap().iter().flatten() {
            db.iter_geo(f);
        }
    }

    /// Walk the ASN databases.
    pub fn iter_asn(&self, f: &mut dyn FnMut(&str, u32)) {
        for db in self.asn.read().unwrap().iter().flatten() {
            db.iter_asn(f);
        }
    }
}
