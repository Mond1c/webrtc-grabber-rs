use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::protocol::PeerStatus;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct PeersResponse {
    pub peers: Vec<PeerStatus>,
}

pub async fn get_peers(State(state): State<Arc<AppState>>) -> Json<PeersResponse> {
    let peers = state.storage.get_all_statuses();
    Json(PeersResponse { peers })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub sfu_id: String,
    pub publishers: usize,
    pub subscribers: usize,
}

pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    use sfu_core::Sfu;

    Json(HealthResponse {
        status: "ok".to_string(),
        sfu_id: state.sfu.id().to_string(),
        publishers: state.storage.get_all_statuses().len(),
        subscribers: 0, // TODO: track subscribers in storage
    })
}
