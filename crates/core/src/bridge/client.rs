//! Generic outbound bridge client (dev-plan/44 Tier 0/1).
//!
//! The reusable half of the LINE/Messenger bridges: dial **out** to a
//! cloud relay over a WebSocket, pump inbound [`WsEnvelope`]s to a sink,
//! reconnect with exponential backoff, and POST replies back. Generic
//! over any [`BridgeConfig`] — LINE, Messenger, and the phone-home
//! channel are all just a `BridgeClient<TheirConfig>`.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::protocol::{parse_envelope, QuickReplyButton, ReplyBody, WsEnvelope, WsIncoming};
use super::BridgeConfig;

#[derive(Debug, thiserror::Error)]
pub enum BridgeClientError {
    #[error("connect: {0}")]
    Connect(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("reply http: {0}")]
    ReplyHttp(String),
    #[error("reply status {status}: {body}")]
    ReplyStatus { status: u16, body: String },
    #[error("cancelled")]
    Cancelled,
}

/// Where inbound envelopes go. Implemented per-channel to drive an agent
/// turn (and reply via the client).
pub trait BridgeEnvelopeSink: Send + Sync + 'static {
    fn on_envelope(&self, envelope: WsEnvelope) -> impl std::future::Future<Output = ()> + Send;
}

/// Outbound WS + reply client over a relay, generic over the channel config.
pub struct BridgeClient<C: BridgeConfig> {
    config: C,
    http: reqwest::Client,
    cancel: Option<crate::cancel::CancelToken>,
    /// Short log tag, e.g. "line" / "messenger" / "phone-home".
    tag: &'static str,
}

impl<C: BridgeConfig> BridgeClient<C> {
    pub fn new(config: C, tag: &'static str) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .user_agent(concat!("thclaws-core/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client build"),
            cancel: None,
            tag,
        }
    }

    pub fn with_cancel(mut self, token: crate::cancel::CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    pub fn config(&self) -> &C {
        &self.config
    }

    /// Send a final reply for `request_id`. The binding JWT is added as
    /// `Authorization: Bearer`.
    pub async fn send_reply(
        &self,
        request_id: &str,
        text: impl Into<String>,
    ) -> Result<(), BridgeClientError> {
        self.post_reply(
            self.config.reply_url(request_id),
            ReplyBody {
                text: text.into(),
                quick_reply: None,
            },
        )
        .await
    }

    /// Reply with tappable buttons (the relay maps them to the channel's
    /// native quick-reply shape).
    pub async fn send_reply_with_buttons(
        &self,
        request_id: &str,
        text: impl Into<String>,
        buttons: Vec<QuickReplyButton>,
    ) -> Result<(), BridgeClientError> {
        self.post_reply(
            self.config.reply_url(request_id),
            ReplyBody {
                text: text.into(),
                quick_reply: Some(buttons),
            },
        )
        .await
    }

    /// Unsolicited push (approval prompt / notice) — no inbound event.
    pub async fn push(&self, text: impl Into<String>) -> Result<(), BridgeClientError> {
        self.post_reply(
            self.config.push_url(),
            ReplyBody {
                text: text.into(),
                quick_reply: None,
            },
        )
        .await
    }

    /// Fan a live chat envelope (assistant_delta / tool_call_start /
    /// user_message / turn_done / …) to the relay so a connected surface
    /// mirrors the conversation. POSTs the JSON to `{prefix}/event` with
    /// the binding JWT. (dev-plan/44 streaming)
    pub async fn push_event(&self, event: serde_json::Value) -> Result<(), BridgeClientError> {
        let resp = self
            .http
            .post(self.config.event_url())
            .bearer_auth(self.config.binding_token())
            .json(&event)
            .send()
            .await
            .map_err(|e| BridgeClientError::ReplyHttp(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(BridgeClientError::ReplyStatus { status, body });
        }
        Ok(())
    }

    /// Tell the relay to drop this binding (best-effort, before local
    /// cleanup on disconnect). POSTs the binding JWT to `/unpair`.
    pub async fn unpair(&self) -> Result<(), BridgeClientError> {
        let resp = self
            .http
            .post(self.config.unpair_url())
            .bearer_auth(self.config.binding_token())
            .send()
            .await
            .map_err(|e| BridgeClientError::ReplyHttp(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(BridgeClientError::ReplyStatus { status, body });
        }
        Ok(())
    }

    async fn post_reply(&self, url: String, body: ReplyBody) -> Result<(), BridgeClientError> {
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.config.binding_token())
            .json(&body)
            .send()
            .await
            .map_err(|e| BridgeClientError::ReplyHttp(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(BridgeClientError::ReplyStatus { status, body });
        }
        Ok(())
    }

    /// Run until cancelled — open the WS, dispatch envelopes to `sink`,
    /// reconnect with exponential backoff (1s → 60s) on any transport
    /// error. Exits with `Cancelled` only when the cancel token fires.
    pub async fn run<S: BridgeEnvelopeSink>(&self, sink: S) -> Result<(), BridgeClientError> {
        let ws_url = self.config.ws_url();
        eprintln!("[{}] client starting → {}", self.tag, redact_token(&ws_url));

        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        loop {
            if self.is_cancelled() {
                return Err(BridgeClientError::Cancelled);
            }
            match self.connect_and_pump(&ws_url, &sink).await {
                Ok(()) => {
                    eprintln!("[{}] ws closed cleanly; reconnecting", self.tag);
                    backoff = Duration::from_secs(1);
                    if self.sleep_with_cancel(Duration::from_secs(1)).await {
                        return Err(BridgeClientError::Cancelled);
                    }
                }
                Err(BridgeClientError::Cancelled) => return Err(BridgeClientError::Cancelled),
                Err(e) => {
                    eprintln!("[{}] ws failed: {e}; backoff {backoff:?}", self.tag);
                    if self.sleep_with_cancel(backoff).await {
                        return Err(BridgeClientError::Cancelled);
                    }
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    async fn connect_and_pump<S: BridgeEnvelopeSink>(
        &self,
        ws_url: &str,
        sink: &S,
    ) -> Result<(), BridgeClientError> {
        let (ws, resp) = connect_async(ws_url)
            .await
            .map_err(|e| BridgeClientError::Connect(e.to_string()))?;
        eprintln!("[{}] ws connected (status {})", self.tag, resp.status());
        let (mut sink_ws, mut stream_ws) = ws.split();

        loop {
            tokio::select! {
                _ = self.cancelled() => {
                    let _ = sink_ws.send(Message::Close(None)).await;
                    return Err(BridgeClientError::Cancelled);
                }
                msg = stream_ws.next() => match msg {
                    Some(Ok(Message::Text(text))) => match parse_envelope(&text) {
                        WsIncoming::Envelope(env) => sink.on_envelope(env).await,
                        WsIncoming::Unknown(raw) => eprintln!("[{}] unknown envelope: {raw}", self.tag),
                        WsIncoming::Closed => return Ok(()),
                    },
                    Some(Ok(Message::Ping(p))) => {
                        if sink_ws.send(Message::Pong(p)).await.is_err() {
                            return Ok(());
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        eprintln!("[{}] ws received close", self.tag);
                        return Ok(());
                    }
                    Some(Ok(_)) => { /* ignore binary/pong */ }
                    Some(Err(e)) => return Err(BridgeClientError::Transport(e.to_string())),
                    None => return Ok(()),
                }
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|t| t.is_cancelled())
    }

    async fn cancelled(&self) {
        if let Some(t) = self.cancel.as_ref() {
            t.cancelled().await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    /// Sleep `dur`, returning `true` if cancelled mid-sleep.
    async fn sleep_with_cancel(&self, dur: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(dur) => false,
            _ = self.cancelled() => true,
        }
    }
}

/// Redact the `?token=…` query so logged WS URLs don't leak the JWT.
fn redact_token(url: &str) -> String {
    match url.split_once("token=") {
        Some((head, _)) => format!("{head}token=<redacted>"),
        None => url.to_string(),
    }
}
