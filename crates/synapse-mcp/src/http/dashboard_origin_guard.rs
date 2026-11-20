//! #167: Origin/Host guard for the credential-free `/dashboard/*` routes.
//!
//! The dashboard has no token by policy (#892/#913), so a browser page on any
//! other origin could otherwise drive it: a body-less cross-site `POST`, a
//! `no-cors` fetch, or `new WebSocket("ws://127.0.0.1:<port>/...")` all reach
//! the daemon with a loopback `Host`. This guard is transport hardening, not
//! authentication:
//!
//! * `Host` must be a loopback name on the daemon's own port (DNS rebinding).
//! * A present `Origin` must be the daemon's own origin, for every method.
//!
//! Requests with NO `Origin` are allowed: browsers always send `Origin` on
//! cross-site POST/fetch/WebSocket, while local non-browser clients (scripts,
//! the tray overlay, tools) do not. Synapse stays open to local use; only
//! cross-site browser requests are refused.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(super) const HOST_REFUSED: &str = "DASHBOARD_HOST_REFUSED";
pub(super) const ORIGIN_REFUSED: &str = "DASHBOARD_ORIGIN_REFUSED";

#[derive(Clone, Copy, Debug)]
pub(super) struct DashboardOriginGuard {
    port: u16,
}

impl DashboardOriginGuard {
    pub(super) const fn new(port: u16) -> Self {
        Self { port }
    }

    /// Returns the refusal code when the request must not reach a dashboard route.
    pub(super) fn check(&self, headers: &HeaderMap) -> Result<(), &'static str> {
        let host = header_str(headers, &header::HOST).ok_or(HOST_REFUSED)?;
        if !self.is_own_authority(host) {
            return Err(HOST_REFUSED);
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            let origin = origin.to_str().map_err(|_| ORIGIN_REFUSED)?;
            if !self.is_own_origin(origin) {
                return Err(ORIGIN_REFUSED);
            }
        }
        Ok(())
    }

    fn is_own_origin(&self, origin: &str) -> bool {
        let Some((scheme, authority)) = origin.trim().split_at_checked("http://".len()) else {
            return false;
        };
        scheme.eq_ignore_ascii_case("http://") && self.is_own_authority(authority)
    }

    fn is_own_authority(&self, authority: &str) -> bool {
        let authority = authority.trim();
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            let Some((host, after)) = rest.split_once(']') else {
                return false;
            };
            (host, after.strip_prefix(':'))
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        matches!(
            host.to_ascii_lowercase().as_str(),
            "127.0.0.1" | "localhost" | "::1"
        ) && port.and_then(|port| port.parse::<u16>().ok()) == Some(self.port)
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

pub(super) async fn require_dashboard_origin(
    State(guard): State<DashboardOriginGuard>,
    request: Request,
    next: Next,
) -> Response {
    match guard.check(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(code) => {
            tracing::warn!(
                code,
                method = %request.method(),
                path = %request.uri().path(),
                "dashboard request refused by Origin/Host guard"
            );
            (StatusCode::FORBIDDEN, code).into_response()
        }
    }
}

#[cfg(test)]
#[path = "tests/dashboard_origin_csrf_167.rs"]
mod dashboard_origin_csrf_167;
