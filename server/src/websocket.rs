use axum::extract::ws::{Message, WebSocket};
use futures::{stream::SplitStream, SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{trace, warn};

use crate::error::{Result, SignallingError};

#[derive(Clone)]
pub struct WsSession {
    pub id: String,
    sender: mpsc::UnboundedSender<Message>,
}

impl WsSession {
    pub fn new(socket: WebSocket, id: String) -> (Self, SplitStream<WebSocket>) {
        let (ws_sender, ws_receiver) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        let id_clone = id.clone();

        tokio::spawn(async move {
            let mut ws_sender = ws_sender;
            while let Some(msg) = rx.recv().await {
                if let Err(e) = ws_sender.send(msg).await {
                    warn!("Failed to send WebSocket message to {}: {}", id_clone, e);
                    break;
                }
            }
            trace!("WebSocket sender task for {} terminated", id_clone);
        });

        (Self { id, sender: tx }, ws_receiver)
    }

    pub fn send_json<T: Serialize>(&self, msg: &T) -> Result<()> {
        let text = serde_json::to_string(msg)?;
        self.sender
            .send(Message::Text(text))
            .map_err(|e| SignallingError::WebSocket(format!("Failed to queue message: {}", e)))
    }

    pub fn send_text(&self, text: String) -> Result<()> {
        self.sender
            .send(Message::Text(text))
            .map_err(|e| SignallingError::WebSocket(format!("Failed to queue message: {}", e)))
    }

    pub fn close(&self) -> Result<()> {
        self.sender
            .send(Message::Close(None))
            .map_err(|e| SignallingError::WebSocket(format!("Failed to send close: {}", e)))
    }
}
