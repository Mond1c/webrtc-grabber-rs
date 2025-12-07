use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

pub type IceCandidateSender = mpsc::UnboundedSender<RTCIceCandidateInit>;

#[async_trait]
pub trait Sfu: Send + Sync {
    fn id(&self) -> &str;

    async fn add_publisher(&self, req: PublisherRequest) -> Result<PublisherResponse>;

    async fn update_publisher(
        &self,
        req: PublisherUpdateRequest,
    ) -> Result<PublisherUpdateResponse>;

    async fn remove_publisher(&self, publisher_id: &str) -> Result<()>;

    async fn add_publisher_ice(
        &self,
        publisher_id: &str,
        candidate: RTCIceCandidateInit,
    ) -> Result<()>;

    async fn add_subscriber(&self, req: SubscriberRequest) -> Result<SubscriberResponse>;

    async fn update_subscriber(
        &self,
        req: SubscriberUpdateRequest,
    ) -> Result<SubscriberUpdateResponse>;

    async fn remove_subscriber(&self, subscriber_id: &str) -> Result<()>;

    async fn add_subscriber_ice(
        &self,
        subscriber_id: &str,
        candidate: RTCIceCandidateInit,
    ) -> Result<()>;

    async fn get_metrics(&self) -> Result<sfu_proto::SfuMetrics>;

    async fn health_check(&self) -> Result<()>;
}

pub struct PublisherRequest {
    pub publisher_id: String,
    pub session_id: String,
    pub offer: RTCSessionDescription,
    pub ice_candidate_tx: Option<IceCandidateSender>,
}

#[derive(Debug)]
pub struct PublisherResponse {
    pub answer: RTCSessionDescription,
    pub publisher_id: String,
}

#[derive(Debug)]
pub struct PublisherUpdateRequest {
    pub publisher_id: String,
    pub offer: RTCSessionDescription,
}

#[derive(Debug)]
pub struct PublisherUpdateResponse {
    pub answer: RTCSessionDescription,
}

pub struct SubscriberRequest {
    pub subscriber_id: String,
    pub publisher_id: String,
    pub offer: RTCSessionDescription,
    pub ice_candidate_tx: Option<IceCandidateSender>,
}

#[derive(Debug)]
pub struct SubscriberResponse {
    pub answer: RTCSessionDescription,
}

#[derive(Debug)]
pub struct SubscriberUpdateRequest {
    pub subscriber_id: String,
}

#[derive(Debug)]
pub struct SubscriberUpdateResponse {
    pub success: bool,
}
