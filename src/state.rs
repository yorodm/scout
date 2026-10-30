use crate::store::Store;
use crate::worker::Job;
use std::sync::Arc;
use tokio::sync::{mpsc::UnboundedSender, RwLock};

pub struct AppState {
    pub store: Arc<RwLock<Store>>,
    pub jobs: UnboundedSender<Job>,
    /// Used to point the demo project at the built-in mock decisions endpoint.
    pub self_url: String,
}
