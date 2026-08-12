//! Phone-home channel (dev-plan/44 Tier 1) — "thClaws… phone home." 👽
//!
//! The local engine dials **out** to thClaws.cloud over the shared
//! [`crate::bridge`] transport and binds to the user's **cloud account**
//! (not a messaging-platform OA). The cloud relay can then route a remote
//! surface — the cloud dashboard's "talk to my local agent", or any
//! registered inbound adapter — down the tunnel to this engine, with no
//! public IP or inbound port.
//!
//! This module currently provides the channel's [`PhoneHomeConfig`] and a
//! [`build_client`] helper over the generic [`BridgeClient`]. The agent
//! session sink, the `--phone-home` entry point, and the cloud-side relay
//! routes land in the follow-up slices (they need the live relay to build +
//! verify end-to-end).

pub mod config;
// `bootstrap` + `session` forward into the GUI worker (`shared_session`,
// itself `#[cfg(feature = "gui")]`), so they're gui-gated too — exactly
// like `line::bootstrap`. The CLI binary has no worker to forward into.
#[cfg(feature = "gui")]
pub mod bootstrap;
#[cfg(feature = "gui")]
pub mod session;

#[cfg(feature = "gui")]
pub use bootstrap::{spawn, PhoneHomeHandle};
pub use config::PhoneHomeConfig;

use crate::bridge::BridgeClient;
use crate::cancel::CancelToken;

/// Short log tag for this channel's [`BridgeClient`].
pub const TAG: &str = "phone-home";

/// Build a phone-home [`BridgeClient`] bound to the cloud account in
/// `config`. Call `.run(sink)` on the result to open the tunnel and pump
/// inbound envelopes; cancel via the shared [`CancelToken`].
pub fn build_client(config: PhoneHomeConfig, cancel: CancelToken) -> BridgeClient<PhoneHomeConfig> {
    BridgeClient::new(config, TAG).with_cancel(cancel)
}

#[derive(Debug, thiserror::Error)]
pub enum PhoneHomePairError {
    #[error("pair request failed: {0}")]
    Http(String),
    #[error("relay rejected pairing ({status}): {body}")]
    Status { status: u16, body: String },
    #[error("pair response malformed: {0}")]
    Decode(String),
    #[error("save binding: {0}")]
    Save(#[from] std::io::Error),
}

#[derive(serde::Deserialize)]
struct PairResponse {
    token: String,
}

/// Exchange a thClaws.cloud CLI token for a phone-home binding: POST the
/// token to the relay's `/ph/pair`, then save the returned binding JWT to
/// `.thclaws/phone-home.json` (in the current dir). Returns the saved
/// config, ready to hand to [`spawn`]. dev-plan/44 Tier 1.
pub async fn pair(
    relay_base: &str,
    cloud_token: &str,
    machine_label: Option<String>,
) -> Result<PhoneHomeConfig, PhoneHomePairError> {
    let base = relay_base.trim_end_matches('/').to_string();
    let url = format!("{base}/ph/pair");
    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .json(&serde_json::json!({
            "token": cloud_token,
            "machine_label": machine_label,
        }))
        .send()
        .await
        .map_err(|e| PhoneHomePairError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(PhoneHomePairError::Status { status, body });
    }
    let parsed: PairResponse = resp
        .json()
        .await
        .map_err(|e| PhoneHomePairError::Decode(e.to_string()))?;

    let config = PhoneHomeConfig {
        binding_token: parsed.token,
        server_url: Some(base),
        machine_label,
    };
    config.save()?;
    Ok(config)
}
