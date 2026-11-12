use std::collections::VecDeque;
use std::fs;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;

use crate::store::config_dir;

pub const PROMPT: &str = "You are a translator. If the text is Chinese, translate to English. If ANY other language NOT Chinese, translate to Simplified CHINESE. output translation(s):";
const ENDPOINT: &str = "https://api.deepseek.com/chat/completions";

#[derive(Clone, Debug)]
pub struct TranslateResult {
    pub translation: String,
    pub cached: bool,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: String,
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    max_tokens: u32,
    temperature: u8,
}

pub struct Translator {
    client: Client,
    key: Option<String>,
    cache: Mutex<VecDeque<(String, String)>>,
}

impl Translator {
    pub fn new() -> Result<Self> {
        let key = api_key();
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build http client")?;
        Ok(Self {
            client,
            key,
            cache: Mutex::new(VecDeque::new()),
        })
    }

    pub fn translate(&self, text: &str) -> Result<TranslateResult> {
        let text = text.trim();
        if text.is_empty() {
            bail!("no input");
        }
        {
            let cache = self.cache.lock().expect("cache lock");
            if let Some((_, value)) = cache.iter().find(|(key, _)| key == text) {
                return Ok(TranslateResult {
                    translation: value.clone(),
                    cached: true,
                });
            }
        }
        let key = self
            .key
            .as_ref()
            .context("DEEPSEEK_API_KEY not set (env or ~/.config/aitr/deepseek)")?;
        let body = Request {
            model: "deepseek-chat",
            messages: vec![Message {
                role: "user",
                content: format!("{PROMPT}\n{text}"),
            }],
            max_tokens: 1024,
            temperature: 0,
        };
        let response = self
            .client
            .post(ENDPOINT)
            .bearer_auth(key)
            .json(&body)
            .send()
            .context("translate request")?;
        let status = response.status();
        let text_body = response.text().context("read translate response")?;
        if !status.is_success() {
            let detail: String = text_body.chars().take(300).collect();
            bail!("translate HTTP {}: {}", status.as_u16(), detail);
        }
        let value: Value = serde_json::from_str(&text_body).context("parse translate response")?;
        let translation = value
            .get("choices")
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("message"))
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("translate returned empty result")?
            .to_string();
        let mut cache = self.cache.lock().expect("cache lock");
        if cache.len() >= 256 {
            cache.pop_front();
        }
        cache.push_back((text.to_string(), translation.clone()));
        Ok(TranslateResult {
            translation,
            cached: false,
        })
    }
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
