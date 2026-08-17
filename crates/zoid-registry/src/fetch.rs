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
/// - Ollama: `POST {base_url}/api/show` → `model_info.context_length`
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
            let v: serde_json::Value = serde_json::from_str(&body)?;
            let window = v
                .get("model_info")
                .and_then(|m| m.get("context_length"))
                .and_then(|n| n.as_u64());
            Ok(window.map(|w| ModelInfo {
                context_window: w,
                max_output: 0,
                tools: true,
                prompt_cache: true,
                thinking: zoid_model::ThinkingSupport::None,
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
}