//! Qwen-Image provider (dev-plan/40) — Alibaba DashScope.
//!
//! qwen-image-2.0 / -pro generate via the DashScope multimodal-generation
//! endpoint, which handles BOTH text→image (a single `{text}` content
//! part) and image→image editing (one or more `{image}` parts + a
//! `{text}` instruction — multi-image editing is supported natively).
//!
//! International endpoint `dashscope-intl.aliyuncs.com`, or
//! `<gateway>/dashscope` when only the thClaws Gateway key is present.
//! Auth is `Authorization: Bearer <DASHSCOPE_API_KEY>`. The response
//! carries a signed image URL (PNG, expires 24h) at
//! `output.choices[0].message.content[].image`, which we download.
//!
//! Pricing is PER IMAGE (2.0 $0.035 / -pro $0.075; 3.0 $0.03 / -pro $0.075
//! at the 2K tile sizes we request), unlike the
//! token-metered chat models — recorded as `price_per_image_usd` in the
//! catalogue. Gateway per-image metering is a follow-up; desktop users
//! with their own DASHSCOPE_API_KEY work today.

use crate::error::{Error, Result};
use crate::media::provider::{ImageModelInfo, ImageProvider, ImageRequest, ImageResult};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const DASHSCOPE_BASE: &str = "https://dashscope-intl.aliyuncs.com";
const GEN_PATH: &str = "/api/v1/services/aigc/multimodal-generation/generation";

const MODELS: &[ImageModelInfo] = &[
    ImageModelInfo {
        id: "qwen-image-2.0",
        aliases: &["qwen", "qwen-image", "qwen-image-2.0"],
        label: "Qwen Image 2.0",
    },
    ImageModelInfo {
        id: "qwen-image-2.0-pro",
        aliases: &["qwen-pro", "qwen-image-2.0-pro"],
        label: "Qwen Image 2.0 Pro",
    },
    ImageModelInfo {
        id: "qwen-image-3.0",
        aliases: &["qwen-3", "qwen-image-3.0"],
        label: "Qwen Image 3.0",
    },
    ImageModelInfo {
        id: "qwen-image-3.0-pro",
        aliases: &["qwen-3-pro", "qwen-image-3.0-pro"],
        label: "Qwen Image 3.0 Pro",
    },
];

/// True for the 3.x series, whose accepted dimensions top out at 2048 per
/// side — unlike 2.x, which takes the wider 2688 tiles below.
fn is_v3(model: &str) -> bool {
    model.starts_with("qwen-image-3")
}

pub struct QwenImageProvider;

impl QwenImageProvider {
    /// Map the engine's portable aspect tiers onto DashScope `W*H` size
    /// strings.
    ///
    /// Both series bill by pixel area — anything over 2,250,000 px is the
    /// "2K" tier — and every tile below stays in it, so the aspect choice
    /// never silently changes what a picture costs. The 3.x series accepts
    /// no side longer than 2048, so it gets its own tiles rather than the
    /// wider 2688-px ones 2.x uses.
    fn size(req: &ImageRequest) -> &'static str {
        if is_v3(&req.model) {
            return match req.aspect_ratio.as_str() {
                "1:1" => "2048*2048",
                "9:16" => "1152*2048",
                "3:4" => "1536*2048",
                "4:3" => "2048*1536",
                _ => "2048*1152", // 16:9 default
            };
        }
        match req.aspect_ratio.as_str() {
            "1:1" => "2048*2048",
            "9:16" => "1536*2688",
            "3:4" => "1728*2368",
            "4:3" => "2368*1728",
            _ => "2688*1536", // 16:9 default
        }
    }
}

#[async_trait]
impl ImageProvider for QwenImageProvider {
    fn id(&self) -> &'static str {
        "qwen"
    }
    fn models(&self) -> &'static [ImageModelInfo] {
        MODELS
    }
    fn resolve_model(&self, raw: &str) -> Option<String> {
        let raw = raw.trim();
        for m in MODELS {
            if raw == m.id || m.aliases.contains(&raw) {
                return Some(m.id.to_string());
            }
        }
        // Forward-compat: accept any future `qwen-image*` id verbatim.
        if raw.starts_with("qwen-image") {
            return Some(raw.to_string());
        }
        None
    }

    async fn generate(&self, req: &ImageRequest) -> Result<ImageResult> {
        let ep = crate::media::provider::resolve_endpoint(
            &["DASHSCOPE_API_KEY"],
            DASHSCOPE_BASE,
            "dashscope",
        )?;
        let size = Self::size(req);

        // content: image parts first (data URIs for local bytes), then
        // the text instruction — text2image is just the text part.
        let mut content: Vec<Value> = Vec::new();
        for img in &req.input_images {
            content.push(json!({
                "image": format!("data:{};base64,{}", img.mime, B64.encode(&img.bytes))
            }));
        }
        content.push(json!({ "text": req.prompt }));

        let body = json!({
            "model": req.model,
            "input": { "messages": [ { "role": "user", "content": content } ] },
            "parameters": {
                "size": size,
                "n": 1,
                "negative_prompt": "",
                "watermark": false
            }
        });
        let url = format!("{}{}", ep.base_url.trim_end_matches('/'), GEN_PATH);
        let client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| Error::Tool(format!("http client: {e}")))?;
        let resp = crate::multi_tenant::attach_member(client.post(&url))
            .bearer_auth(&ep.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Tool(format!("qwen http: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(Error::Tool(format!(
                "qwen http {status}: {}",
                b.chars().take(400).collect::<String>()
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| Error::Tool(format!("qwen response not json: {e}")))?;

        // Walk output.choices[].message.content[] for the first `image`
        // URL (signed OSS URL, PNG).
        let img_url = v
            .pointer("/output/choices")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|choice| {
                choice
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
            })
            .flatten()
            .find_map(|part| part.get("image").and_then(|i| i.as_str()))
            .ok_or_else(|| {
                let raw = v.to_string();
                Error::Tool(format!(
                    "qwen returned no image — raw: {}",
                    raw.chars().take(500).collect::<String>()
                ))
            })?
            .to_string();

        let img = client
            .get(&img_url)
            .send()
            .await
            .map_err(|e| Error::Tool(format!("qwen image download: {e}")))?;
        if !img.status().is_success() {
            return Err(Error::Tool(format!(
                "qwen image download http {}",
                img.status()
            )));
        }
        let bytes = img
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| Error::Tool(format!("qwen image body: {e}")))?;
        Ok(ImageResult { bytes })
    }
}
