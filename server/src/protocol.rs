use serde::{Deserialize, Serialize};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PingMessage {
    pub timestamp: i64,
    pub connections_count: Option<u32>,
    pub stream_types: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] 
pub enum PlayerEvent {
    Auth,
    AuthRequest,
    AuthFailed,
    InitPeer,
    Offer,
    OfferFailed,
    Answer,
    PlayerIce,
    Ping,
    Pong,
    PeerStatus,
}


#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerMessage {
    pub event: String,
    
    pub player_auth: Option<PlayerAuth>,
    pub access_message: Option<String>,
    
    pub init_peer: Option<PcConfigMessage>,
    pub offer: Option<OfferMessage>,
    pub ice: Option<IceMessage>,
    pub ping: Option<PingMessage>,
    
    pub peers_status: Option<Vec<PeerStatus>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerAuth {
    pub credential: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferMessage {
    pub sdp: String,
    pub type_: String,
    
    pub peer_id: Option<String>,
    pub peer_name: Option<String>,
    pub stream_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceMessage {
    pub candidate: RTCIceCandidateInit,
    pub peer_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JsonIceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JsonRtcConfiguration {
    pub ice_servers: Vec<JsonIceServer>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcConfigMessage {
    pub pc_config: JsonRtcConfiguration,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrabberMessage {
    pub event: String,
    
    pub init_peer: Option<GrabberInitPeerMessage>,
    pub offer: Option<OfferMessage>,
    pub answer: Option<OfferMessage>,
    pub ice: Option<IceMessage>,
    pub ping: Option<PingMessage>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrabberInitPeerMessage {
    pub pc_config: JsonRtcConfiguration,
    pub ping_interval: u64,
}


#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub name: String,
    pub socket_id: String,
    pub online: bool,
    pub connections: u32,
    pub stream_types: Vec<String>,
    pub last_ping: i64,
}
