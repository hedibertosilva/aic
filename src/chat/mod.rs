//! Local web chat: HTTP server, persisted conversations, agent quotas and
//! consumption tracking. The page lives in `web/`.

pub mod consumption;
pub mod quotas;
pub mod server;
pub mod store;
