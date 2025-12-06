use axum::{
    extract::{ConnectInfo, Path, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::IntoResponse,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use futures::{sink::SinkExt, stream::{SplitSink, StreamExt}};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{info, error, warn, instrument};
use anyhow::{Result, anyhow, Context};
use serde::Serialize;

use webrtc::peer_connection::sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

use sfu_core::{PublisherRequest, Sfu, SubscriberRequest};
use sfu_local::config::SfuConfig;
use crate::{
    protocol::{self, *},
    storage::Storage,
};

pub struct AppState {
    pub sfu: Box<dyn Sfu + Send + Sync>, 
    pub storage: Storage,
    pub config: Arc<SfuConfig>,
}

impl AppState {
    pub fn get_client_rtc_config(&self) -> protocol::JsonRtcConfiguration {
        let ice_servers = self.config.ice_servers.iter()
            .map(|url| protocol::JsonIceServer {
                urls: vec![url.clone()],
                username: None,
                credential: None,
            })
            .collect();

        protocol::JsonRtcConfiguration { ice_servers }
    }
}

struct WsSession {
    id: String,
    session_id: String,
    sender: mpsc::UnboundedSender<Message>,
}

impl WsSession {
    fn new(socket: WebSocket, id: String, session_id: String) -> (Self, futures::stream::SplitStream<WebSocket>) {
        let (mut ws_sender, ws_receiver) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        let id_clone = id.clone();

        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let Err(e) = ws_sender.send(msg).await {
                    warn!("Failed to send message to {}: {}", id_clone, e);
                    break;
                }
            }
        });

        let tx_ping = tx.clone();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(30)).await;
                let ping_msg = protocol::PlayerMessage {
                    event: "PING".to_string(),
                    ping: Some(protocol::PingMessage {
                        timestamp: chrono::Utc::now().timestamp(),
                        connections_count: None,
                        stream_types: None,
                    }),
                    ..Default::default()
                };
                
                let json = serde_json::to_string(&ping_msg).unwrap_or_default();
                if tx_ping.send(Message::Text(json)).is_err() {
                    break;
                }
            }
        });

        (Self { id, sender: tx, session_id }, ws_receiver)
    }

    fn send_json<T: Serialize>(&self, msg: &T) -> Result<()> {
        let text = serde_json::to_string(msg)
            .context("Failed to serialize message to JSON")?;
        self.sender.send(Message::Text(text))
            .map_err(|e| anyhow!("Failed to send message: {}", e))
    }
}

pub async fn ws_player_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_player_socket(socket, addr, state).await {
            error!("Player socket error: {:?}", e);
        }
    })
}

#[instrument(skip(socket, state), fields(ip = %addr))]
async fn handle_player_socket(socket: WebSocket, addr: SocketAddr, state: Arc<AppState>) -> Result<()> {
    let socket_id = addr.to_string();
    info!("Player connected");

    let (session, mut receiver) = WsSession::new(socket, socket_id.clone(), socket_id.clone());

    session.send_json(&protocol::PlayerMessage {
        event: "AUTH_REQUEST".to_string(),
        ..Default::default()
    })?;

    let auth_msg = tokio::time::timeout(Duration::from_secs(5), receiver.next()).await
        .context("Auth timeout")?
        .ok_or_else(|| anyhow!("Stream closed during auth"))??;

    let is_authenticated = match auth_msg {
        Message::Text(text) => {
            let msg: protocol::PlayerMessage = serde_json::from_str(&text)?;
            msg.event == "AUTH" && 
                msg.player_auth.map(|a| state.config.validate_credentials(&a.credential)).unwrap_or(false)
        },
        _ => false,
    };

    if !is_authenticated {
        session.send_json(&protocol::PlayerMessage {
            event: "AUTH_FAILED".to_string(),
            access_message: Some("Invalid credentials".to_string()),
            ..Default::default()
        })?;
        return Ok(());
    }

    session.send_json(&protocol::PlayerMessage {
        event: "INIT_PEER".to_string(),
        init_peer: Some(protocol::PcConfigMessage {
            pc_config: state.get_client_rtc_config(),
        }),
        ..Default::default()
    })?;

    while let Some(result) = receiver.next().await {
        let msg = result?;
        if let Message::Text(text) = msg {
            let parsed = match serde_json::from_str::<protocol::PlayerMessage>(&text) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to parse player message: {}", e);
                    continue;
                }
            };

            process_player_message(&session, parsed, &state).await?;
        }
    }

    info!("Player disconnected");
    let _ = state.sfu.remove_subscriber(&socket_id).await;
    
    Ok(())
}

async fn process_player_message(session: &WsSession, msg: protocol::PlayerMessage, state: &Arc<AppState>) -> Result<()> {
    match msg.event.as_str() {
        "OFFER" => {
            if let Some(offer_data) = msg.offer {
                let target_peer = offer_data.peer_name.clone().unwrap_or_default();
                
                if let Some(peer_status) = state.storage.get_peer_by_name(&target_peer) {
                    let offer = match RTCSessionDescription::offer(offer_data.sdp) {
                        Ok(offer) => Some(offer),
                        Err(e) => {
                            error!("SFU subscribe error: {}", e);
                            session.send_json(&protocol::PlayerMessage {
                                event: "OFFER_FAILED".to_string(),
                                ..Default::default()
                            })?;
                            None
                        }
                    };
                    if offer.is_none() {
                        return Ok(());
                    }
                    let offer = offer.unwrap();

                    let req = SubscriberRequest {
                        subscriber_id: session.id.clone(),
                        publisher_id: peer_status.socket_id,
                        offer,
                    };

                    match state.sfu.add_subscriber(req).await {
                        Ok(res) => {
                            session.send_json(&protocol::PlayerMessage {
                                event: "ANSWER".to_string(),
                                offer: Some(protocol::OfferMessage {
                                    type_: "answer".to_string(),
                                    sdp: res.answer.sdp,
                                    peer_id: None,
                                    peer_name: None,
                                    stream_type: None,
                                }),
                                ..Default::default()
                            })?;
                        }
                        Err(e) => {
                            error!("SFU subscribe error: {}", e);
                            session.send_json(&protocol::PlayerMessage {
                                event: "OFFER_FAILED".to_string(),
                                ..Default::default()
                            })?;
                        }
                    }
                } else {
                    warn!("Target peer '{}' not found", target_peer);
                    session.send_json(&protocol::PlayerMessage {
                        event: "OFFER_FAILED".to_string(),
                        ..Default::default()
                    })?;
                }
            }
        },
        "PLAYER_ICE" => {
            if let Some(ice_msg) = msg.ice {
                state.sfu.add_subscriber_ice(&session.id, ice_msg.candidate).await?;
            }
        },
        _ => {}
    }
    Ok(())
}

pub async fn ws_grabber_handler(
    ws: WebSocketUpgrade,
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_grabber_socket(socket, addr, name, state).await {
            error!("Grabber error: {:?}", e);
        }
    })
}

#[instrument(skip(socket, state), fields(name = %name, ip = %addr))]
async fn handle_grabber_socket(socket: WebSocket, addr: SocketAddr, name: String, state: Arc<AppState>) -> Result<()> {
    let socket_id = addr.to_string();
    info!("Grabber connected");

    let (session, mut receiver) = WsSession::new(socket, socket_id.clone(), name.clone());

    state.storage.add_peer(name.clone(), socket_id.clone());

    session.send_json(&protocol::GrabberMessage {
        event: "INIT_PEER".to_string(),
        init_peer: Some(protocol::GrabberInitPeerMessage {
            pc_config: state.get_client_rtc_config(),
            ping_interval: 5000,
        }),
        ..Default::default()
    })?;

    while let Some(result) = receiver.next().await {
        let msg = result?;
        if let Message::Text(text) = msg {
             let parsed = match serde_json::from_str::<protocol::GrabberMessage>(&text) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to parse grabber message: {}", e);
                    continue;
                }
            };
            process_grabber_message(&session, parsed, &state).await?;
        }
    }

    info!("Grabber disconnected");
    state.storage.remove_peer_by_socket_id(&socket_id);
    let _ = state.sfu.remove_publisher(&socket_id).await;

    Ok(())
}

async fn process_grabber_message(session: &WsSession, msg: protocol::GrabberMessage, state: &Arc<AppState>) -> Result<()> {
    match msg.event.as_str() {
        "PING" => {
            if let Some(ping) = msg.ping {
                state.storage.update_ping(
                    &session.id,
                    ping.connections_count.unwrap_or(0),
                    ping.stream_types.unwrap_or_default(),
                );
            }
        },
        "OFFER" | "OFFER_ANSWER" => {
            if let Some(offer) = msg.offer.or(msg.answer) {
                let offer = match RTCSessionDescription::offer(offer.sdp) {
                    Ok(offer) => Some(offer),
                    Err(e) => {
                        error!("SFU subscribe error: {}", e);
                        session.send_json(&protocol::PlayerMessage {
                            event: "OFFER_FAILED".to_string(),
                            ..Default::default()
                        })?;
                        None
                    }
                };
                if offer.is_none() {
                    return Ok(());
                }
                let offer = offer.unwrap();

                let req = PublisherRequest {
                    session_id: session.session_id.clone(),
                    publisher_id: session.id.clone(),
                    offer,
                };

                match state.sfu.add_publisher(req).await {
                    Ok(res) => {
                        session.send_json(&protocol::GrabberMessage {
                            event: "ANSWER".to_string(),
                            answer: Some(protocol::OfferMessage {
                                type_: "answer".to_string(),
                                sdp: res.answer.sdp,
                                peer_id: None,
                                peer_name: None,
                                stream_type: None,
                            }),
                            ..Default::default()
                        })?;
                    }
                    Err(e) => error!("SFU add publisher error: {}", e),
                }
            }
        },
        "GRABBER_ICE" => {
            if let Some(ice_msg) = msg.ice {
                state.sfu.add_publisher_ice(&session.id, ice_msg.candidate).await?;
            }
        },
        _ => {}
    }
    Ok(())
}