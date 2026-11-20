use rmcp::{RoleServer, ServerHandler, ServiceExt, service::RequestContext};

struct ContextServer;
impl ServerHandler for ContextServer {}

/// Keep both initialized peers alive until the last request-context clone drops.
struct ContextPeers {
    _server: rmcp::service::RunningService<RoleServer, ContextServer>,
    _client: rmcp::service::RunningService<rmcp::RoleClient, ()>,
}

pub(crate) async fn request_context_for_session(session_id: &str) -> RequestContext<RoleServer> {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server, client) = tokio::join!(ContextServer.serve(server_io), ().serve(client_io));
    let (server, client) = match (server, client) {
        (Ok(server), Ok(client)) => (server, client),
        _ => panic!("isolated rmcp initialization failed"),
    };
    let mut context = RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        server.peer().clone(),
    );
    let mut request = axum::http::Request::new(());
    let header = match axum::http::HeaderValue::from_str(session_id) {
        Ok(header) => header,
        Err(error) => panic!("invalid fixture session header: {error}"),
    };
    request.headers_mut().insert("Mcp-Session-Id", header);
    context.extensions.insert(request.into_parts().0);
    context.extensions.insert(std::sync::Arc::new(ContextPeers {
        _server: server,
        _client: client,
    }));
    context
}
