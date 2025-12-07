use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use sfu_core::SubscriberRequest;

use crate::error::{Result, SignallingError};
use crate::protocol::{self, PlayerMessage};
use crate::state::AppState;
use crate::websocket::WsSession;

pub async fn ws_player_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_player_connection(socket, addr, state).await {
            error!("Player connection error from {}: {:?}", addr, e);
        }
    })
}

#[instrument(skip(socket, state), fields(ip = %addr))]
async fn handle_player_connection(
    socket: WebSocket,
    addr: SocketAddr,
    state: Arc<AppState>,
) -> Result<()> {
    let session_id = format!("player-{}", addr);
    info!("Player connecting");

    let (session, mut receiver) = WsSession::new(socket, session_id.clone());

    session.send_json(&PlayerMessage {
        event: "AUTH_REQUEST".to_string(),
        ..Default::default()
    })?;

    let auth_msg = tokio::time::timeout(Duration::from_secs(10), receiver.next())
        .await
        .map_err(|_| SignallingError::Timeout("Authentication timeout".to_string()))?
        .ok_or_else(|| SignallingError::SessionError("Connection closed during auth".to_string()))?
        .map_err(|e| SignallingError::WebSocket(format!("WebSocket error: {}", e)))?;

    if !authenticate_player(&auth_msg, &state)? {
        session.send_json(&PlayerMessage {
            event: "AUTH_FAILED".to_string(),
            access_message: Some("Invalid credentials".to_string()),
            ..Default::default()
        })?;
        return Err(SignallingError::AuthenticationFailed(
            "Invalid credentials".to_string(),
        ));
    }

    session.send_json(&PlayerMessage {
        event: "INIT_PEER".to_string(),
        init_peer: Some(protocol::PcConfigMessage {
            pc_config: state.get_client_rtc_config(),
        }),
        ..Default::default()
    })?;

    info!("Player authenticated and initialized");

    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                if let Err(e) = handle_player_message(&session, &text, &state).await {
                    warn!("Error processing player message: {}", e);
                }
            }
            Ok(Message::Close(_)) => {
                info!("Player closed connection");
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = session.send_text(format!("{{\"event\":\"PONG\"}}"));
            }
            Err(e) => {
                warn!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    info!("Player disconnected");
    let _ = state.sfu.remove_subscriber(&session_id).await;

    Ok(())
}

fn authenticate_player(msg: &Message, state: &AppState) -> Result<bool> {
    let Message::Text(text) = msg else {
        return Ok(false);
    };

    let player_msg: PlayerMessage = serde_json::from_str(text)
        .map_err(|e| SignallingError::InvalidMessageFormat(e.to_string()))?;

    Ok(player_msg.event == "AUTH"
        && player_msg
            .player_auth
            .map(|a| state.config.validate_credentials(&a.credential))
            .unwrap_or(false))
}

async fn handle_player_message(session: &WsSession, text: &str, state: &AppState) -> Result<()> {
    let msg: PlayerMessage = serde_json::from_str(text)
        .map_err(|e| SignallingError::InvalidMessageFormat(e.to_string()))?;

    match msg.event.as_str() {
        "OFFER" => handle_subscribe_offer(session, msg, state).await,
        "PLAYER_ICE" => handle_player_ice(session, msg, state).await,
        "PING" => {
            session.send_json(&PlayerMessage {
                event: "PONG".to_string(),
                ..Default::default()
            })?;
            Ok(())
        }
        _ => {
            warn!("Unknown player event: {}", msg.event);
            Ok(())
        }
    }
}

async fn handle_subscribe_offer(
    session: &WsSession,
    msg: PlayerMessage,
    state: &AppState,
) -> Result<()> {
    let offer_data = msg
        .offer
        .ok_or_else(|| SignallingError::InvalidMessageFormat("Missing offer data".to_string()))?;

    let target_peer = offer_data
        .peer_name
        .ok_or_else(|| SignallingError::InvalidMessageFormat("Missing peer_name".to_string()))?;

    let peer_status = state
        .storage
        .get_peer_by_name(&target_peer)
        .ok_or_else(|| SignallingError::PeerNotFound(target_peer.clone()))?;

    let offer = RTCSessionDescription::offer(offer_data.sdp)
        .map_err(|e| SignallingError::InvalidMessageFormat(format!("Invalid SDP offer: {}", e)))?;

    let (ice_tx, mut ice_rx) = mpsc::unbounded_channel();
    let session_for_ice = session.clone();

    tokio::spawn(async move {
        while let Some(candidate) = ice_rx.recv().await {
            let _ = session_for_ice.send_json(&PlayerMessage {
                event: "SERVER_ICE".to_string(),
                ice: Some(protocol::IceMessage {
                    candidate,
                    peer_id: None,
                }),
                ..Default::default()
            });
        }
    });

    let req = SubscriberRequest {
        subscriber_id: session.id.clone(),
        publisher_id: peer_status.socket_id,
        offer,
        ice_candidate_tx: Some(ice_tx),
    };

    match state.sfu.add_subscriber(req).await {
        Ok(res) => {
            session.send_json(&PlayerMessage {
                event: "ANSWER".to_string(),
                offer: Some(protocol::OfferMessage {
                    type_: "answer".to_string(),
                    sdp: res.answer.sdp,
                    peer_id: None,
                    peer_name: Some(target_peer),
                    stream_type: None,
                }),
                ..Default::default()
            })?;
            Ok(())
        }
        Err(e) => {
            error!("SFU subscribe error: {}", e);
            session.send_json(&PlayerMessage {
                event: "OFFER_FAILED".to_string(),
                ..Default::default()
            })?;
            Err(SignallingError::SfuError(e))
        }
    }
}

async fn handle_player_ice(
    session: &WsSession,
    msg: PlayerMessage,
    state: &AppState,
) -> Result<()> {
    let ice_msg = msg
        .ice
        .ok_or_else(|| SignallingError::InvalidMessageFormat("Missing ICE data".to_string()))?;

    state
        .sfu
        .add_subscriber_ice(&session.id, ice_msg.candidate)
        .await
        .map_err(SignallingError::SfuError)?;

    Ok(())
}
