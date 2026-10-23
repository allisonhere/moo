pub mod client;
mod event_hub;
pub(crate) mod handoff;
pub mod schema;
mod server;
mod status;
mod subscriptions;
mod wait;

pub use event_hub::EventHub;
pub use server::ServerHandle;
pub(crate) use server::{api_method_name, start_server_with_stop_control};
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::api::schema::Request;

pub const SOCKET_PATH_ENV_VAR: &str = "MOO_SOCKET_PATH";

/// Serializes an API value. These types always serialize, but a failure must still
/// produce a well-formed error response instead of panicking the server.
pub(crate) fn json_string<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to serialize API value");
        r#"{"error":{"code":"encode_failed","message":"failed to encode response"}}"#.to_owned()
    })
}

pub(crate) fn request_changes_ui(request: &Request) -> bool {
    request.method.operation().changes_ui
}

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
    pub response_write_complete: Option<std::sync::mpsc::Receiver<()>>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

pub fn socket_path() -> PathBuf {
    crate::session::active_api_socket_path()
}
