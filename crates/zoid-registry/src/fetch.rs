//! Per-provider live-list fetchers and response parsers.
//!
//! The parse functions are pure and testable without network access; the
//! `list_models` / `caps` functions are async and drive `reqwest` against the
//! provider's live endpoints. These are the primitives the refresh tool
//! (Task 13) uses to query live endpoints and reconcile the on-disk registry.

use anyhow::Result;
use zoid_model::ModelInfo;

/// Parse Ollama `/api/tags` → `.models[].name`.
pub fn parse_ollama_tags(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse OpenAI-compat/Anthropic `/v1/models` → `.data[].id`.
pub fn parse_data_id(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse Gemini `/v1/models` → `.models[].name` (strip the `models/` prefix).
pub fn parse_gemini_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    m.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.trim_start_matches("models/").to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the context window from an Ollama `/api/show` response body. The
/// `model_info` map carries family-specific keys like `glm.context_length`,
/// `llama.context_length`, etc. — we try known keys and fall back to any
/// key ending in `.context_length`. Uses `as_f64()` since JSON numbers may
/// be floats. Returns `None` when unparseable. Mirrors the
/// `parse_ollama_context_window` in `zoid-provider::ollama`.
pub fn parse_ollama_context_window(body: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let info = v.get("model_info")?;
    for key in &[
        "glm.context_length",
        "llama.context_length",
        "deepseek.context_length",
        "qwen.context_length",
        "mistral.context_length",
    ] {
        if let Some(n) = info.get(key).and_then(|v| v.as_f64()) {
            return Some(n as u64);
        }
    }
    if let Some(obj) = info.as_object() {
        for (k, v) in obj {
            if k.ends_with(".context_length") {
                if let Some(n) = v.as_f64() {
                    return Some(n as u64);
                }
            }
        }
    }
    None
}

/// Parse the Ollama `/api/show` `capabilities` array for thinking support.
/// Returns `ThinkingSupport::Toggle` when the array contains `"thinking"`,
/// `None` otherwise (including absent, non-array, null, or malformed). Mirrors
/// `parse_ollama_thinking` in `zoid-provider::ollama`.
pub fn parse_ollama_thinking(body: &str) -> zoid_model::ThinkingSupport {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return zoid_model::ThinkingSupport::None,
    };
    let caps = match v.get("capabilities").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return zoid_model::ThinkingSupport::None,
    };
    if caps.iter().any(|c| c.as_str() == Some("thinking")) {
        zoid_model::ThinkingSupport::Toggle
    } else {
        zoid_model::ThinkingSupport::None
    }
}

/// Parse Gemini `/v1beta/models` caps for one model → `ModelInfo`.
///
/// Looks for the entry whose `name` (after stripping the `models/` prefix)
/// matches `model`, then reads `inputTokenLimit`/`outputTokenLimit`.
pub fn parse_gemini_caps(body: &str, model: &str) -> Option<ModelInfo> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let arr = v.get("models")?.as_array()?;
    let m = arr.iter().find(|m| {
        m.get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.trim_start_matches("models/"))
            == Some(model)
    })?;
    Some(ModelInfo {
        context_window: m
            .get("inputTokenLimit")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        max_output: m
            .get("outputTokenLimit")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        tools: true,
        prompt_cache: false,
        thinking: zoid_model::ThinkingSupport::Toggle,
        thinking_wire: zoid_model::ThinkingWireShape::None,
    })
}

/// Fetch the live model id list for a provider.
///
/// Provider-specific endpoints and auth headers:
/// - Ollama (cloud/local): `{base_url}/api/tags`, `Authorization: Bearer {key}`
/// - Anthropic: `{base_url}/v1/models`, `x-api-key: {key}` (+ `anthropic-version`)
/// - Gemini: `{base_url}/v1/models`, `x-goog-api-key: {key}`
/// - zai-coding-plan: `{base_url}/models`, `Authorization: Bearer {key}`
/// - Other OpenAI-compat: `{base_url}/v1/models`, `Authorization: Bearer {key}`
pub async fn list_models(provider_id: &str, base_url: &str, key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let (url, auth_header, auth_value) = match provider_id {
        "ollama-cloud" | "ollama-local" => (
            format!("{base_url}/api/tags"),
            "authorization",
            format!("Bearer {key}"),
        ),
        "anthropic-api" => (format!("{base_url}/v1/models"), "x-api-key", key.to_string()),
        "gemini-api" => (format!("{base_url}/v1/models"), "x-goog-api-key", key.to_string()),
        "zai-coding-plan" => (format!("{base_url}/models"), "authorization", format!("Bearer {key}")),
        _ => (
            format!("{base_url}/v1/models"),
            "authorization",
            format!("Bearer {key}"),
        ),
    };
    let mut req = client.get(&url).header(auth_header, &auth_value);
    if provider_id == "anthropic-api" {
        req = req.header("anthropic-version", "2023-06-01");
    }
    let body = req.send().await?.error_for_status()?.text().await?;
    Ok(match provider_id {
        "ollama-cloud" | "ollama-local" => parse_ollama_tags(&body),
        "gemini-api" => parse_gemini_models(&body),
        _ => parse_data_id(&body),
    })
}

/// Fetch wire-derived caps for a model.
///
/// Only Ollama and Gemini expose a caps endpoint we can derive `ModelInfo`
/// from; other providers return `None` (caps come from the shipped registry).
///
/// - Ollama: `POST {base_url}/api/show` → `model_info.<family>.context_length`
///   (family-prefixed key) plus the `capabilities` array for thinking support
/// - Gemini: `GET {base_url}/v1beta/models` (parsed by `parse_gemini_caps`)
pub async fn caps(
    provider_id: &str,
    base_url: &str,
    key: &str,
    model: &str,
) -> Result<Option<ModelInfo>> {
    let client = reqwest::Client::new();
    match provider_id {
        "ollama-cloud" | "ollama-local" => {
            let body = client
                .post(format!("{base_url}/api/show"))
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .json(&serde_json::json!({ "model": model }))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let body = client
                .post(format!("{base_url}/api/show"))
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .json(&serde_json::json!({ "model": model }))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let window = parse_ollama_context_window(&body);
            let thinking = parse_ollama_thinking(&body);
            Ok(window.map(|w| ModelInfo {
                context_window: w,
                max_output: 0,
                tools: true,
                prompt_cache: true,
                thinking,
                thinking_wire: zoid_model::ThinkingWireShape::None,
            }))
        }
        "gemini-api" => {
            let body = client
                .get(format!("{base_url}/v1beta/models"))
                .header("x-goog-api-key", key)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            Ok(parse_gemini_caps(&body, model))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ollama_tags_shape() {
        let body = r#"{"models":[{"name":"glm-5.2:cloud"},{"name":"llama3"}]}"#;
        assert_eq!(parse_ollama_tags(body), vec!["glm-5.2:cloud", "llama3"]);
    }

    #[test]
    fn parse_data_id_shape() {
        let body = r#"{"data":[{"id":"gpt-5.4"},{"id":"gpt-5"}]}"#;
        assert_eq!(parse_data_id(body), vec!["gpt-5.4", "gpt-5"]);
    }

    #[test]
    fn parse_gemini_models_shape() {
        let body = r#"{"models":[{"name":"models/gemini-3-flash"}]}"#;
        assert_eq!(parse_gemini_models(body), vec!["gemini-3-flash"]);
    }

    #[test]
    fn parse_gemini_caps_shape() {
        let body = r#"{"models":[{"name":"models/gemini-3-flash","inputTokenLimit":1000000,"outputTokenLimit":8192}]}"#;
        let caps = parse_gemini_caps(body, "gemini-3-flash").unwrap();
        assert_eq!(caps.context_window, 1_000_000);
        assert_eq!(caps.max_output, 8192);
    }

    #[test]
    fn parse_ollama_context_window_family_key() {
        // Ollama returns family-prefixed keys; the bare `context_length` key
        // does NOT exist, so the old `model_info.context_length` lookup missed.
        let body = r#"{"model_info":{"glm.context_length":131072,"llama.context_length":8192}}"#;
        assert_eq!(parse_ollama_context_window(body), Some(131_072));
    }

    #[test]
    fn parse_ollama_context_window_float_number() {
        // JSON numbers may be floats; as_u64() would miss them.
        let body = r#"{"model_info":{"llama.context_length":32768.0}}"#;
        assert_eq!(parse_ollama_context_window(body), Some(32_768));
    }

    #[test]
    fn parse_ollama_context_window_fallback_scan() {
        // Unknown family falls back to any *.context_length key.
        let body = r#"{"model_info":{"phi4.context_length":16384}}"#;
        assert_eq!(parse_ollama_context_window(body), Some(16_384));
    }

    #[test]
    fn parse_ollama_context_window_missing() {
        assert_eq!(parse_ollama_context_window(r#"{"model_info":{}}"#), None);
        assert_eq!(parse_ollama_context_window("not json"), None);
    }

    #[test]
    fn parse_ollama_thinking_toggle() {
        let body = r#"{"capabilities":["tools","thinking"]}"#;
        assert_eq!(
            parse_ollama_thinking(body),
            zoid_model::ThinkingSupport::Toggle
        );
    }

    #[test]
    fn parse_ollama_thinking_none_when_absent() {
        let body = r#"{"capabilities":["tools"]}"#;
        assert_eq!(
            parse_ollama_thinking(body),
            zoid_model::ThinkingSupport::None
        );
    }

    #[test]
    fn parse_ollama_thinking_none_when_malformed() {
        assert_eq!(
            parse_ollama_thinking("not json"),
            zoid_model::ThinkingSupport::None
        );
        assert_eq!(
            parse_ollama_thinking(r#"{"capabilities":"not-array"}"#),
            zoid_model::ThinkingSupport::None
        );
    }
}