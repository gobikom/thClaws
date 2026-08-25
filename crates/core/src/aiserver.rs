//! Discovery of a co-located AI Server (the DGX Spark appliance).
//!
//! thClaws ships on the same box as AI Server and starts with `--serve`, so
//! it should find the local LLM gateway by itself rather than making the
//! operator paste a base URL. AI Server answers that in one request:
//!
//! ```text
//! GET http://127.0.0.1:9000/.well-known/aiserver
//! ```
//!
//! The endpoint replies **only to loopback** (403 otherwise), so a 200 with
//! `product == "aiserver"` is proof we are on the same machine — no
//! heuristics, no config. See `docs/AISERVER-API-GUIDE.md` §1-5.
//!
//! What we do with it: point the `litellm` provider at the gateway
//! (`endpoints.openai`) and default the model to `litellm/auto`, which the
//! gateway routes to whichever model the user activated in the hub. Nothing
//! the user configured is touched — see [`apply_to_env`].

use serde::Deserialize;
use std::sync::OnceLock;

/// One entry of the discovery document's `endpoints` map.
#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub up: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Endpoints {
    /// LiteLLM gateway, OpenAI-shaped (`:4000/v1`). The route we take.
    pub openai: Endpoint,
    /// Same gateway, Anthropic-shaped (`:4000`, no `/v1`).
    #[serde(default)]
    pub anthropic: Option<Endpoint>,
    /// The engine itself (`:8000/v1`) — lowest latency, no routing/`auto`.
    #[serde(default)]
    pub direct: Option<Endpoint>,
}

/// The discovery document. Unknown fields are ignored on purpose: §7 of the
/// guide promises fields may be *added*, never removed or retyped.
#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub product: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hardware: String,
    pub endpoints: Endpoints,
    /// Model id to send. Always `"auto"` today — the gateway resolves it to
    /// whatever the user activated, so thClaws never learns real model names.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_loaded: bool,
    #[serde(default)]
    pub hub: String,
    #[serde(default)]
    pub machine_id: String,
}

impl Discovery {
    /// The model id to hand the `litellm` provider, e.g. `litellm/auto`.
    pub fn model_id(&self) -> String {
        let name = self.model.as_deref().unwrap_or("auto");
        format!("litellm/{name}")
    }

    /// True when the gateway is up *and* a model is loaded. `up` is a
    /// snapshot — the gateway restarts for ~5-15s whenever the user swaps
    /// models — so callers should treat `false` as "retry", never as
    /// "there is no AI Server here".
    pub fn ready(&self) -> bool {
        self.endpoints.openai.up && self.model_loaded
    }
}

static CACHED: OnceLock<Option<Discovery>> = OnceLock::new();

/// Hub base URL. Overridable so tests (and an operator debugging a
/// non-standard port) can retarget it; the real appliance always answers on
/// loopback :9000.
fn hub_base() -> String {
    std::env::var("THCLAWS_AISERVER_HUB")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:9000".to_string())
}

/// Probe once and remember the answer for the rest of the process. Safe to
/// call from anywhere; only the first call does I/O.
///
/// Skipped entirely when `LITELLM_BASE_URL` is already set — the user (or
/// their shell / `endpoints.json` / `.env`) has pointed the provider
/// somewhere deliberate, and we neither override that nor spend a request
/// finding out we're not allowed to.
pub async fn probe_and_cache() -> Option<&'static Discovery> {
    if CACHED.get().is_none() {
        let found = if std::env::var("LITELLM_BASE_URL").is_ok() {
            None
        } else {
            fetch().await
        };
        let _ = CACHED.set(found);
    }
    cached()
}

/// The cached discovery result, or `None` when we never found an AI Server
/// (or never probed). Never does I/O — sync callers like
/// `preferred_default_model` read through this.
pub fn cached() -> Option<&'static Discovery> {
    CACHED.get().and_then(|o| o.as_ref())
}

async fn fetch() -> Option<Discovery> {
    let url = format!("{}/.well-known/aiserver", hub_base());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let d: Discovery = resp.json().await.ok()?;
    // Guard against something unrelated squatting on :9000.
    (d.product == "aiserver").then_some(d)
}

/// Point the `litellm` provider at the discovered gateway, without
/// disturbing anything already configured.
///
/// Only ever *fills in* unset vars, so the precedence documented in
/// [`crate::endpoints`] still holds: shell export > `endpoints.json` >
/// `.env` > this. Returns true when it set the base URL.
pub fn apply_to_env(d: &Discovery) -> bool {
    let mut set_base = false;
    if std::env::var("LITELLM_BASE_URL").is_err() {
        std::env::set_var("LITELLM_BASE_URL", &d.endpoints.openai.base_url);
        set_base = true;
    }
    // The guide is explicit that `sk-local` must not be hardcoded — it is a
    // per-machine value that may change — so it is only ever read from the
    // response.
    if let Some(key) = d.endpoints.openai.api_key.as_deref() {
        if !key.is_empty() && std::env::var("LITELLM_API_KEY").is_err() {
            std::env::set_var("LITELLM_API_KEY", key);
        }
    }
    set_base
}

/// Probe, wire up the environment, and tell the operator what happened.
/// Called once at startup, before any `AppConfig::load`.
pub async fn bootstrap() {
    let Some(d) = probe_and_cache().await else {
        return;
    };
    if apply_to_env(d) {
        eprintln!(
            "\x1b[32m[aiserver] {} v{} on {} — LLM via {}\x1b[0m",
            d.vendor, d.version, d.hardware, d.endpoints.openai.base_url
        );
    }
    if !d.ready() {
        // Not an error: the gateway also reports down for a few seconds
        // whenever the user switches models.
        eprintln!(
            "\x1b[33m[aiserver] no model is loaded yet — open {} to pick one\x1b[0m",
            d.hub
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `THCLAWS_AISERVER_HUB` is process-global, so the two probe tests must
    // not overlap or one clears the other's target mid-request.
    static HUB_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn doc(product: &str) -> serde_json::Value {
        serde_json::json!({
            "product": product,
            "vendor": "AIServer.in.th",
            "version": "0.2.76",
            "hardware": "dgx-spark-gb10",
            "endpoints": {
                "openai":    {"base_url": "http://127.0.0.1:4000/v1", "api_key": "sk-local", "up": true},
                "anthropic": {"base_url": "http://127.0.0.1:4000", "api_key": "sk-local", "up": true},
                "direct":    {"base_url": "http://127.0.0.1:8000/v1", "up": true,
                              "note": "ต่อตรง engine"}
            },
            "model": "auto",
            "model_loaded": true,
            "hub": "http://127.0.0.1:9000",
            "machine_id": "2b50a0679bab64bc"
        })
    }

    #[test]
    fn parses_the_documented_response_and_ignores_unknown_fields() {
        let mut v = doc("aiserver");
        // §7: new fields may appear at any time; they must not break parsing.
        v["license"] = serde_json::json!({"tier": "pro"});
        v["models_available"] = serde_json::json!(["a", "b"]);
        let d: Discovery = serde_json::from_value(v).expect("parses");
        assert_eq!(d.endpoints.openai.base_url, "http://127.0.0.1:4000/v1");
        assert_eq!(d.endpoints.openai.api_key.as_deref(), Some("sk-local"));
        assert_eq!(d.model_id(), "litellm/auto");
        assert!(d.ready());
        assert_eq!(d.machine_id, "2b50a0679bab64bc");
    }

    /// `direct` carries no `api_key`; the struct must not require one.
    #[test]
    fn direct_endpoint_without_a_key_still_parses() {
        let d: Discovery = serde_json::from_value(doc("aiserver")).expect("parses");
        let direct = d.endpoints.direct.expect("direct present");
        assert!(direct.api_key.is_none());
    }

    #[test]
    fn a_loaded_model_is_required_for_ready() {
        let mut v = doc("aiserver");
        v["model_loaded"] = serde_json::json!(false);
        let d: Discovery = serde_json::from_value(v).expect("parses");
        assert!(!d.ready(), "no model loaded yet");
    }

    #[tokio::test]
    async fn fetch_rejects_another_service_squatting_on_the_port() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/aiserver"))
            .respond_with(ResponseTemplate::new(200).set_body_json(doc("something-else")))
            .mount(&server)
            .await;

        let _guard = HUB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("THCLAWS_AISERVER_HUB", server.uri());
        let got = fetch().await;
        std::env::remove_var("THCLAWS_AISERVER_HUB");
        assert!(got.is_none(), "product != aiserver must not count");
    }

    #[tokio::test]
    async fn fetch_reads_a_real_response() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/aiserver"))
            .respond_with(ResponseTemplate::new(200).set_body_json(doc("aiserver")))
            .mount(&server)
            .await;

        let _guard = HUB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("THCLAWS_AISERVER_HUB", server.uri());
        let got = fetch().await;
        std::env::remove_var("THCLAWS_AISERVER_HUB");
        assert_eq!(got.expect("found").model_id(), "litellm/auto");
    }
}
