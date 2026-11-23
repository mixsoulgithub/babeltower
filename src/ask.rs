use anyhow::{Result, bail};

use crate::llm;

pub const PROMPT: &str = "You are a terse technical assistant embedded in a text editor. \
Answer the user's text directly and concisely. If it is a shell command or code, explain what it \
does plus the key options. If it is a question, answer it. Reply in Simplified Chinese, keep code \
and identifiers as-is, use short markdown, no preamble and no restating the input.";

/// 把选中的文本直接当作问题/待解释内容发给模型
pub fn ask(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("no input");
    }
    llm::chat(Some(PROMPT), text, 1200)
}
