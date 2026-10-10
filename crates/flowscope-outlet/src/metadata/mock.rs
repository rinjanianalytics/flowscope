// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Mock provider for tests. Port of `outlet/metadata/tests.go`.

use super::{Answer, Exporter, Interface, Provider, ProviderError, Query};
use flowscope_schema::InterfaceBoundary;
use futures::future::BoxFuture;

/// A provider answering for any exporter. Interface 999 is unknown, 998
/// gives an error, 1010 and 2010 come with exporter metadata (2010 also
/// with interface metadata).
pub struct MockProvider;

impl Provider for MockProvider {
    fn query(&self, query: Query) -> BoxFuture<'_, Result<Answer, ProviderError>> {
        Box::pin(async move {
            match query.if_index {
                999 => return Ok(Answer::default()),
                998 => return Err(ProviderError::Other("noooo".into())),
                _ => {}
            }
            let mut answer = Answer {
                found: true,
                exporter: Exporter {
                    name: flowscope_common::helpers::unmap_ip(&query.exporter_ip).replace('.', "_"),
                    ..Default::default()
                },
                interface: Interface {
                    name: format!("Gi0/0/{}", query.if_index),
                    description: format!("Interface {}", query.if_index),
                    speed: 1000,
                    ..Default::default()
                },
            };
            if query.if_index == 1010 || query.if_index == 2010 {
                answer.exporter.group = "metadata group".into();
                answer.exporter.region = "metadata region".into();
                answer.exporter.role = "metadata role".into();
                answer.exporter.site = "metadata site".into();
                answer.exporter.tenant = "metadata tenant".into();
            }
            if query.if_index == 2010 {
                answer.interface.boundary = InterfaceBoundary::External;
                answer.interface.connectivity = "metadata connectivity".into();
                answer.interface.provider = "metadata provider".into();
            }
            Ok(answer)
        })
    }
}
