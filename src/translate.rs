use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::{Result, bail};

use crate::llm;

pub const PROMPT: &str = "You are a translator. If the text is Chinese, translate to English. If ANY other language NOT Chinese, translate to Simplified CHINESE. output translation(s):";

#[derive(Clone, Debug)]
pub struct TranslateResult {
    pub translation: String,
    pub cached: bool,
}

pub struct Translator {
    cache: Mutex<VecDeque<(String, String)>>,
}

impl Translator {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(VecDeque::new()),
        }
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
        let translation = llm::chat(Some(PROMPT), text, 1024)?;
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
