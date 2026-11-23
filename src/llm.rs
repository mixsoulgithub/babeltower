use std::fs;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;

use crate::store::config_dir;

pub const ENDPOINT: &str = "https://api.deepseek.com/chat/completions";
const MODEL: &str = "deepseek-flash";

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    max_tokens: u32,
    temperature: u8,
    // 关闭思维链：问答/翻译都要快，不要 reasoning tokens
    thinking: Thinking,
}

/// 单轮对话：可选 system 提示词 + 用户内容，返回助手回复文本
pub fn chat(system: Option<&str>, user: &str, max_tokens: u32) -> Result<String> {
    let key = api_key().context("DEEPSEEK_API_KEY not set (env or ~/.config/bbt/deepseek)")?;
    let mut messages = Vec::with_capacity(2);
    if let Some(system) = system {
        messages.push(Message {
            role: "system",
            content: system,
        });
    }
    messages.push(Message {
        role: "user",
        content: user,
    });
    let body = Request {
        model: MODEL,
        messages,
        max_tokens,
        temperature: 0,
        thinking: Thinking { kind: "disabled" },
    };
    let response = client()?
        .post(ENDPOINT)
        .bearer_auth(key)
        .json(&body)
        .send()
        .context("chat request")?;
    let status = response.status();
    let text_body = response.text().context("read chat response")?;
    if !status.is_success() {
        let detail: String = text_body.chars().take(300).collect();
        bail!("chat HTTP {}: {}", status.as_u16(), detail);
    }
    let value: Value = serde_json::from_str(&text_body).context("parse chat response")?;
    value
        .get("choices")
        .and_then(|value| value.get(0))
        .and_then(|value| value.get("message"))
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("chat returned empty result")
        .map(str::to_string)
}

fn client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("build http client")
}

pub fn api_key() -> Option<String> {
    if let Ok(value) = std::env::var("DEEPSEEK_API_KEY") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    fs::read_to_string(config_dir().join("deepseek"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
