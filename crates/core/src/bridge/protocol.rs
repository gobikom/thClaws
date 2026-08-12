//! Canonical wire shapes for the bridge transport (dev-plan/44).
//!
//! These are the JSON frames the cloud relay pushes over the WebSocket
//! (`WsEnvelope`) and the body posted back to `{prefix}/reply/{id}`
//! (`ReplyBody`). They are channel-agnostic — LINE, Messenger, and the
//! phone-home channel all speak this shape. (The LINE/Messenger modules
//! still carry their own byte-compatible copies pending the Tier-0
//! migration onto these.)

use serde::{Deserialize, Serialize};

/// Inbound envelope — what the relay pushes the local engine. `kind`-tagged
/// so future variants deserialise side-by-side without breaking existing ones.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsEnvelope {
    /// A message from the external user → drive an agent turn, reply via
    /// `request_id`.
    UserMessage {
        text: String,
        /// Channel-native reply handle (LINE replyToken, etc.). Empty for
        /// channels that reply purely by `request_id` (phone-home).
        #[serde(default)]
        reply_token: String,
        request_id: String,
        /// Source-native message id, for future dedup. Empty until used.
        #[serde(default)]
        msg_id: String,
    },
    /// Quick-reply / button tap — `data` is the developer-set payload
    /// (e.g. `tool:approve:<id>`), used by the over-bridge approval UX.
    Postback { data: String },
    /// Server-pushed notice (paired, reconnected …). Logged locally, not
    /// forwarded to the agent.
    Notice { text: String },
    /// A file uploaded from a remote surface. Bytes are base64 (WS frames
    /// are text); the engine decodes + writes under `<workspace>/uploads/`.
    Upload {
        filename: String,
        content_b64: String,
        #[serde(default)]
        media_type: Option<String>,
        size_bytes: u64,
        request_id: String,
    },
}

/// Result of decoding a WS text frame.
#[derive(Debug)]
pub enum WsIncoming {
    Envelope(WsEnvelope),
    /// Valid JSON, no known variant.
    Unknown(String),
    /// Server closed the WS cleanly.
    Closed,
}

/// Decode a relay WS text frame into a [`WsIncoming`].
pub fn parse_envelope(text: &str) -> WsIncoming {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return WsIncoming::Closed;
    }
    match serde_json::from_str::<WsEnvelope>(trimmed) {
        Ok(env) => WsIncoming::Envelope(env),
        Err(_) => WsIncoming::Unknown(trimmed.to_string()),
    }
}

/// Body of `POST {prefix}/reply/{request_id}`. The binding JWT goes in the
/// `Authorization: Bearer` header, not this body.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ReplyBody {
    pub text: String,
    /// Optional reply buttons; the relay expands these into the channel's
    /// native quick-reply payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_reply: Option<Vec<QuickReplyButton>>,
}

/// One reply button (channel-agnostic; the relay maps it to the native shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickReplyButton {
    /// Label shown to the user.
    pub label: String,
    /// Payload the tap carries back as a [`WsEnvelope::Postback`].
    pub data: String,
    /// Optional text echoed as the user's "typed" reply on tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_round_trips_and_defaults_optional_fields() {
        let json = r#"{"kind":"user_message","text":"hi","request_id":"r1"}"#;
        match parse_envelope(json) {
            WsIncoming::Envelope(WsEnvelope::UserMessage {
                text,
                request_id,
                reply_token,
                msg_id,
            }) => {
                assert_eq!(text, "hi");
                assert_eq!(request_id, "r1");
                assert_eq!(reply_token, ""); // defaulted
                assert_eq!(msg_id, "");
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn parses_relay_line_server_user_message_frame() {
        // The exact shape line-server's `broker::WsEnvelope::UserMessage`
        // serialises — note `line_msg_id`, which core doesn't model. Locks
        // the cross-crate wire contract: core must ignore `line_msg_id`,
        // read `reply_token`, and default `msg_id`.
        let json = r#"{"kind":"user_message","text":"hi","reply_token":"rt","request_id":"r1","line_msg_id":"m1"}"#;
        match parse_envelope(json) {
            WsIncoming::Envelope(WsEnvelope::UserMessage {
                text,
                reply_token,
                request_id,
                msg_id,
            }) => {
                assert_eq!(text, "hi");
                assert_eq!(reply_token, "rt");
                assert_eq!(request_id, "r1");
                assert_eq!(msg_id, ""); // line_msg_id ignored, msg_id defaulted
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn empty_frame_is_closed_unknown_is_unknown() {
        assert!(matches!(parse_envelope("   "), WsIncoming::Closed));
        assert!(matches!(
            parse_envelope("{\"x\":1}"),
            WsIncoming::Unknown(_)
        ));
    }

    #[test]
    fn reply_body_omits_none_quick_reply() {
        let body = ReplyBody {
            text: "ok".into(),
            quick_reply: None,
        };
        assert_eq!(serde_json::to_string(&body).unwrap(), r#"{"text":"ok"}"#);
    }
}
