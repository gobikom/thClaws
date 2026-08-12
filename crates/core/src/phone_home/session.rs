//! Phone-home session sink (dev-plan/44 Tier 1) — gui-gated because it
//! forwards inbound tunnel messages into the GUI worker's `ShellInput`
//! channel (the same path LINE/Messenger use).
//!
//! [`PhoneHomeSink`] implements [`BridgeEnvelopeSink`]: each inbound
//! [`WsEnvelope::UserMessage`] from the dashboard is injected into the
//! worker as `ShellInput::LineMessage` to drive a turn. The reply isn't
//! posted back here — the worker's ViewEvent fan-out streams the whole
//! turn up `/ph/event` so the dashboard renders it live (dev-plan/44
//! streaming).

use std::sync::mpsc;

use tokio::sync::oneshot;

use crate::bridge::{BridgeEnvelopeSink, WsEnvelope};
use crate::shared_session::ShellInput;

/// Inbound-envelope handler for the phone-home tunnel.
pub struct PhoneHomeSink {
    input_tx: mpsc::Sender<ShellInput>,
}

impl PhoneHomeSink {
    pub fn new(input_tx: mpsc::Sender<ShellInput>) -> Self {
        Self { input_tx }
    }
}

impl BridgeEnvelopeSink for PhoneHomeSink {
    async fn on_envelope(&self, envelope: WsEnvelope) {
        match envelope {
            WsEnvelope::UserMessage { text, .. } => {
                // Inject the dashboard message to drive a turn. We DON'T
                // post the captured reply back via /ph/reply — the worker's
                // ViewEvent fan-out streams the whole turn (user echo +
                // assistant deltas + turn_done) up `/ph/event` instead, so
                // the dashboard renders it live like every other surface
                // (dev-plan/44 streaming). The oneshot is ignored.
                let (respond, _rx) = oneshot::channel();
                let _ = self
                    .input_tx
                    .send(ShellInput::LineMessage { text, respond });
            }
            WsEnvelope::Notice { text } => {
                eprintln!("[phone-home] notice: {text}");
            }
            // Approval-over-bridge (Postback) and Upload land in a
            // follow-up; for now they're ignored so the tunnel stays up.
            WsEnvelope::Postback { .. } | WsEnvelope::Upload { .. } => {}
        }
    }
}
