// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Configuration of the console component.

use crate::query;
use crate::widgets::TopWidget;
use flowscope_common::config::{Duration, Validate};
use serde::{Deserialize, Serialize};
use std::time::Duration as StdDuration;

/// Configuration of the console component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// URL path prefix where the console is hosted ("/flowscope/").
    pub url_prefix: String,
    /// Defaults for the "visualize" tab.
    pub default_visualize_options: VisualizeOptions,
    /// Widgets of the home page.
    pub homepage_top_widgets: Vec<TopWidget>,
    /// Filter for the top widgets, written for the source direction.
    pub homepage_top_widgets_filter: query::Filter,
    /// Filter for the graph of the home page.
    pub homepage_graph_filter: query::Filter,
    /// Time range of the graph of the home page.
    pub homepage_graph_time_range: Duration,
    /// Upper limit of the number of dimensions to return.
    pub dimensions_limit: i64,
    /// Branding of the console.
    pub branding: bool,
    /// How long to keep the costly requests in cache.
    #[serde(rename = "cache-ttl")]
    pub cache_ttl: Duration,
}

/// Defaults for the "visualize" tab. The names are the ones of the API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct VisualizeOptions {
    pub graph_type: String,
    pub start: String,
    pub end: String,
    pub filter: String,
    pub dimensions: Vec<query::Column>,
    pub limit: i64,
    pub limit_type: String,
    pub bidirectional: bool,
    pub previous_period: bool,
}

impl Default for VisualizeOptions {
    fn default() -> Self {
        VisualizeOptions {
            graph_type: "stacked".into(),
            start: "6 hours ago".into(),
            end: "now".into(),
            filter: "InIfBoundary = external".into(),
            dimensions: vec![query::Column::new("SrcAS")],
            limit: 10,
            limit_type: "avg".into(),
            bidirectional: false,
            previous_period: false,
        }
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            url_prefix: "/".into(),
            default_visualize_options: VisualizeOptions::default(),
            homepage_top_widgets: vec![
                TopWidget::SrcAS,
                TopWidget::SrcPort,
                TopWidget::Protocol,
                TopWidget::SrcCountry,
                TopWidget::Etype,
            ],
            homepage_top_widgets_filter: query::Filter::new("InIfBoundary = external"),
            homepage_graph_filter: query::Filter::new("InIfBoundary = external"),
            homepage_graph_time_range: StdDuration::from_secs(24 * 3600).into(),
            dimensions_limit: 50,
            branding: false,
            cache_ttl: StdDuration::from_secs(3 * 3600).into(),
        }
    }
}

impl Validate for Configuration {
    fn validate(&self) -> Result<(), String> {
        let v = &self.default_visualize_options;
        if !["stacked", "stacked100", "lines", "grid", "heatmap", "sankey"].contains(&v.graph_type.as_str()) {
            return Err(format!("DefaultVisualizeOptions.GraphType: invalid value {:?}", v.graph_type));
        }
        if v.start.is_empty() || v.end.is_empty() {
            return Err("DefaultVisualizeOptions: Start and End are required".into());
        }
        if v.limit < 5 {
            return Err("DefaultVisualizeOptions.Limit: should be at least 5".into());
        }
        if !["avg", "max", "last"].contains(&v.limit_type.as_str()) {
            return Err(format!("DefaultVisualizeOptions.LimitType: invalid value {:?}", v.limit_type));
        }
        if *self.homepage_graph_time_range < StdDuration::from_secs(60) {
            return Err("HomepageGraphTimeRange: should be at least 1m".into());
        }
        if self.dimensions_limit < 10 {
            return Err("DimensionsLimit: should be at least 10".into());
        }
        if *self.cache_ttl < StdDuration::from_secs(5) {
            return Err("CacheTTL: should be at least 5s".into());
        }
        Ok(())
    }
}

impl Configuration {
    /// The URL prefix, always starting and ending with "/".
    pub fn url_prefix(&self) -> String {
        let prefix = &self.url_prefix;
        if prefix.is_empty() || prefix == "/" {
            return "/".into();
        }
        let mut prefix = prefix.clone();
        if !prefix.starts_with('/') {
            prefix.insert(0, '/');
        }
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        prefix
    }
}
