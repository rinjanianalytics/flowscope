// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `version` command.

use flowscope_common::helpers::{FLOWSCOPE_VERSION, compiler_version};
use flowscope_schema::Component;

pub fn run(debug: bool) -> anyhow::Result<()> {
    println!("flowscope {FLOWSCOPE_VERSION}");
    println!("  Built with: {}", compiler_version());
    if !debug {
        return Ok(());
    }
    println!();
    let schema = Component::default_schema();
    let primary = schema.clickhouse_primary_keys();
    let mut columns: Vec<_> =
        schema.all_columns().iter().filter(|c| c.key < flowscope_schema::ColumnKey::Last).collect();
    columns.sort_by_key(|c| c.key);
    println!("Can be disabled:");
    for c in &columns {
        if !c.disabled && !c.no_disable && !primary.contains(&c.name) {
            println!("- {}", c.name);
        }
    }
    println!();
    println!("Can be enabled:");
    for c in &columns {
        if c.disabled {
            if c.clickhouse_main_only {
                println!("- {} (main table only)", c.name);
            } else {
                println!("- {}", c.name);
            }
        }
    }
    Ok(())
}
