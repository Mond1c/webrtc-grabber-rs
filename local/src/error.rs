use thiserror::Error;

#[derive(Debug, Error)]
pub enum SfuError {
    #[error("Publisher not found: {0}")]
    PublisherNotFound(String),

    #[error("Subscriber not found: {0}")]
    SubscriberNotFound(String),

    #[error("Track not found: {0}")]
    TrackNotFound(String),

    #[error("Failed to create peer connection: {0}")]
    PeerConnectionCreation(String),

    #[error("Failed to set remote description: {0}")]
    SetRemoteDescription(String),

    #[error("Failed to create answer: {0}")]
    CreateAnswer(String),

    #[error("Failed to set local description: {0}")]
    SetLocalDescription(String),

    #[error("Failed to add ICE candidate: {0}")]
    AddIceCandidate(String),

    #[error("Failed to add track: {0}")]
    AddTrack(String),

    #[error("WebRTC error: {0}")]
    WebRtc(#[from] webrtc::Error),

    #[error("Broadcaster channel is full, packet dropped")]
    BroadcastChannelFull,

    #[error("Broadcaster channel closed")]
    BroadcastChannelClosed,

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, SfuError>;
