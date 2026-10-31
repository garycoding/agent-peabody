// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The HTTP client shared by the three services, and the bearer token each may need.

use crate::settings::Endpoint;
use std::time::Duration;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("the HTTP client could not be built")
}

/// A request to `path` on `ep`, carrying its token where it has one.
pub fn request(
    c: &reqwest::Client,
    method: reqwest::Method,
    ep: &Endpoint,
    path: &str,
) -> reqwest::RequestBuilder {
    let rb = c.request(method, ep.at(path));
    if ep.token.is_empty() {
        rb
    } else {
        rb.bearer_auth(&ep.token)
    }
}

/// A failed request, said briefly enough for the window's status line.
pub fn describe(e: &reqwest::Error) -> String {
    if e.is_connect() {
        "not reachable".into()
    } else if e.is_timeout() {
        "timed out".into()
    } else if let Some(s) = e.status() {
        format!("answered {s}")
    } else {
        e.to_string()
    }
}
