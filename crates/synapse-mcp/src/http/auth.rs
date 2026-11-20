use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode, Uri, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const TOKEN_ENV: &str = "SYNAPSE_BEARER_TOKEN";
const APPDATA_ENV: &str = "APPDATA";
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

#[derive(Clone, Debug)]
pub(super) struct HttpAuth {
    token_digest: [u8; 32],
    source: TokenSource,
    bind_addr: SocketAddr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenSource {
    File(PathBuf),
    Env,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum AuthFailure {
    Missing,
    Malformed,
    Invalid,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum OriginFailure {
    HostMissing,
    HostMalformed,
    HostRefused,
    OriginMissingNonLoopback,
    OriginMalformed,
    OriginRefused,
}

impl HttpAuth {
    pub(super) fn load(bind_addr: SocketAddr) -> anyhow::Result<Self> {
        let (token, source) = load_token()?;
        Ok(Self {
            token_digest: digest_token(&token),
            source,
            bind_addr,
        })
    }

    /// Test seam: an `HttpAuth` for a known token (source `env`), so a test
    /// can build the authenticated router without the token file or env.
    #[cfg(test)]
    pub(super) fn from_token(token: &str, bind_addr: SocketAddr) -> Self {
        Self {
            token_digest: digest_token(token),
            source: TokenSource::Env,
            bind_addr,
        }
    }

    pub(super) const fn source_label(&self) -> &'static str {
        match self.source {
            TokenSource::File(_) => "file",
            TokenSource::Env => "env",
        }
    }

    pub(super) fn authorize(&self, headers: &HeaderMap) -> Result<(), AuthFailure> {
        let token = bearer_token(headers)?;
        if self.token_matches(token) {
            Ok(())
        } else {
            Err(AuthFailure::Invalid)
        }
    }

    pub(super) fn validate_origin_and_host(
        &self,
        headers: &HeaderMap,
    ) -> Result<(), OriginFailure> {
        validate_host(headers)?;
        validate_origin(headers, self.bind_addr)
    }

    pub(super) fn token_matches(&self, candidate: &str) -> bool {
        let candidate_digest = digest_token(candidate);
        bool::from(
            self.token_digest
                .as_slice()
                .ct_eq(candidate_digest.as_slice()),
        )
    }
}

pub(super) async fn require_http_security(
    State(auth): State<Arc<HttpAuth>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if auth.bind_addr.ip().is_loopback()
        && validate_host(request.headers()).is_ok()
        && crate::chrome_debugger_bridge::is_direct_http_extension_bridge_request(
            request.headers(),
            request.uri(),
        )
    {
        return next.run(request).await;
    }
    if let Err(failure) = auth.validate_origin_and_host(request.headers()) {
        return forbidden(failure);
    }
    match auth.authorize(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(AuthFailure::Invalid) => match bearer_token(request.headers()) {
            Ok(token) => {
                let token = token.to_owned();
                authorize_spawn_scoped(&token, request, next).await
            }
            Err(failure) => unauthorized(failure),
        },
        Err(failure) => unauthorized(failure),
    }
}

/// #22: a non-master bearer may be a per-spawn scoped credential, valid only
/// for its own spawn's MCP session, event ingress, and Codex request bridge.
async fn authorize_spawn_scoped(token: &str, request: Request<Body>, next: Next) -> Response {
    use crate::spawn_credentials::{ScopedGrant, ScopedRequest, registry};

    let scoped = ScopedRequest::from_http(request.method(), request.uri(), request.headers());
    let body_check = scoped.requires_body_spawn_id();
    let grant = match registry().authorize(token, &scoped) {
        Ok(grant) => grant,
        Err(refusal) => {
            tracing::warn!(
                code = synapse_core::error_codes::HTTP_TOKEN_INVALID,
                reason = refusal.reason(),
                path = request.uri().path(),
                "HTTP scoped spawn credential rejected"
            );
            return unauthorized(AuthFailure::Invalid);
        }
    };
    let request = if body_check {
        match require_body_spawn_id(request, grant.spawn_id()).await {
            Ok(request) => request,
            Err(response) => return response,
        }
    } else {
        request
    };
    match grant {
        ScopedGrant::Request { .. } => next.run(request).await,
        ScopedGrant::Initialize(claim) => {
            let response = next.run(request).await;
            let session_id = response
                .status()
                .is_success()
                .then(|| response.headers().get(MCP_SESSION_ID_HEADER))
                .flatten()
                .and_then(|value| value.to_str().ok());
            claim.bind(session_id);
            response
        }
    }
}

async fn require_body_spawn_id(
    request: Request<Body>,
    spawn_id: &str,
) -> Result<Request<Body>, Response> {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(
        body,
        crate::server::codex_app_server_bridge::MAX_CODEX_APP_SERVER_REQUEST_BODY_BYTES,
    )
    .await
    .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE.into_response())?;
    let named = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("spawn_id")?.as_str().map(ToOwned::to_owned));
    if named.as_deref() != Some(spawn_id) {
        tracing::warn!(
            code = synapse_core::error_codes::HTTP_TOKEN_INVALID,
            reason = "scoped_credential_body_spawn_mismatch",
            "HTTP scoped spawn credential rejected"
        );
        return Err(unauthorized(AuthFailure::Invalid));
    }
    Ok(Request::from_parts(parts, Body::from(bytes)))
}

/// Load the daemon bearer token value (file or env), for in-process consumers
/// such as the `--mode connect` bridge that must authenticate to the daemon.
pub(crate) fn load_token_value() -> anyhow::Result<String> {
    load_token().map(|(token, _source)| token)
}

fn load_token() -> anyhow::Result<(String, TokenSource)> {
    match token_file_path() {
        Some(path) if path.is_file() => {
            let token = std::fs::read_to_string(&path)
                .with_context(|| format!("read HTTP bearer token file {}", path.display()))?;
            let token = normalize_token(&token)
                .with_context(|| format!("HTTP bearer token file is empty: {}", path.display()))?;
            Ok((token, TokenSource::File(path)))
        }
        Some(_) | None => load_env_token(),
    }
}

fn load_env_token() -> anyhow::Result<(String, TokenSource)> {
    let token = std::env::var(TOKEN_ENV)
        .with_context(|| format!("{TOKEN_ENV} is unset and token.txt is absent"))?;
    let token = normalize_token(&token).with_context(|| format!("{TOKEN_ENV} is empty"))?;
    Ok((token, TokenSource::Env))
}

fn token_file_path() -> Option<PathBuf> {
    let appdata = std::env::var_os(APPDATA_ENV)?;
    Some(PathBuf::from(appdata).join("synapse").join("token.txt"))
}

fn normalize_token(raw: &str) -> anyhow::Result<String> {
    let token = raw.trim();
    if token.is_empty() {
        bail!("empty token")
    }
    Ok(token.to_owned())
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthFailure> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthFailure::Missing)?
        .to_str()
        .map_err(|_| AuthFailure::Malformed)?
        .trim();
    let mut parts = raw.splitn(2, char::is_whitespace);
    let scheme = parts.next().ok_or(AuthFailure::Malformed)?;
    let token = parts.next().ok_or(AuthFailure::Malformed)?.trim();
    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() {
        return Err(AuthFailure::Malformed);
    }
    Ok(token)
}

fn validate_host(headers: &HeaderMap) -> Result<(), OriginFailure> {
    let raw = headers
        .get(header::HOST)
        .ok_or(OriginFailure::HostMissing)?
        .to_str()
        .map_err(|_| OriginFailure::HostMalformed)?
        .trim();
    let authority = Authority::try_from(raw).map_err(|_| OriginFailure::HostMalformed)?;
    if host_allowed(authority.host()) {
        Ok(())
    } else {
        Err(OriginFailure::HostRefused)
    }
}

fn validate_origin(headers: &HeaderMap, bind_addr: SocketAddr) -> Result<(), OriginFailure> {
    let Some(raw) = headers.get(header::ORIGIN) else {
        return if bind_addr.ip().is_loopback() {
            Ok(())
        } else {
            Err(OriginFailure::OriginMissingNonLoopback)
        };
    };
    let raw = raw
        .to_str()
        .map_err(|_| OriginFailure::OriginMalformed)?
        .trim();
    let uri = raw
        .parse::<Uri>()
        .map_err(|_| OriginFailure::OriginMalformed)?;
    if uri.scheme_str() != Some("http") {
        return Err(OriginFailure::OriginRefused);
    }
    let authority = uri.authority().ok_or(OriginFailure::OriginMalformed)?;
    if host_allowed(authority.host()) {
        Ok(())
    } else {
        Err(OriginFailure::OriginRefused)
    }
}

fn host_allowed(host: &str) -> bool {
    matches!(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase()
            .as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn digest_token(token: &str) -> [u8; 32] {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn unauthorized(failure: AuthFailure) -> Response {
    tracing::warn!(
        code = synapse_core::error_codes::HTTP_TOKEN_INVALID,
        reason = ?failure,
        "HTTP bearer token rejected"
    );
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        synapse_core::error_codes::HTTP_TOKEN_INVALID,
    )
        .into_response()
}

fn forbidden(failure: OriginFailure) -> Response {
    tracing::warn!(
        code = synapse_core::error_codes::HTTP_ORIGIN_REFUSED,
        reason = ?failure,
        "HTTP origin or host rejected"
    );
    (
        StatusCode::FORBIDDEN,
        synapse_core::error_codes::HTTP_ORIGIN_REFUSED,
    )
        .into_response()
}
