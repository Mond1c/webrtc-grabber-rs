use dashmap::DashMap;
use std::sync::Arc;
use crate::protocol::PeerStatus;

#[derive(Clone)]
pub struct Storage {
    peers: Arc<DashMap<String, PeerStatus>>,
}

impl Storage {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(DashMap::new()),
        }
    }

    pub fn add_peer(&self, name: String, socket_id: String) {
        self.peers.insert(name.clone(), PeerStatus {
            name,
            socket_id,
            online: true,
            connections: 0,
            stream_types: vec![],
            last_ping: chrono::Utc::now().timestamp(),
        });
    }

    pub fn get_peer_by_name(&self, name: &str) -> Option<PeerStatus> {
        self.peers.get(name).map(|p| p.clone())
    }

    pub fn update_ping(&self, socket_id: &str, connections: u32, streams: Vec<String>) {
        for mut peer in self.peers.iter_mut() {
            if peer.socket_id == socket_id {
                peer.connections = connections;
                peer.stream_types = streams;
                peer.last_ping = chrono::Utc::now().timestamp();
                peer.online = true;
                break;
            }
        }
    }

    pub fn remove_peer_by_socket_id(&self, socket_id: &str) {
        self.peers.retain(|_, v| v.socket_id != socket_id);
    }

    pub fn get_all_statuses(&self) -> Vec<PeerStatus> {
        self.peers.iter().map(|p| p.value().clone()).collect()
    }
}
