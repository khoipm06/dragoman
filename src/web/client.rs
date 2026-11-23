use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER, USER_AGENT};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info};

use crate::session::SessionData;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamChunkPayload {
    pub conversation_id: Option<String>,
    pub response_id: Option<String>,
    pub choice_id: Option<String>,
    pub candidate_text: Option<String>,
}

pub struct GeminiWebClient {
    session: Arc<Mutex<SessionData>>,
    http: reqwest::Client,
}

impl GeminiWebClient {
    pub fn new(session: SessionData) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .unwrap_or_default();

        Self {
            session: Arc::new(Mutex::new(session)),
            http,
        }
    }

    pub fn session(&self) -> Arc<Mutex<SessionData>> {
        self.session.clone()
    }

    pub async fn current_session(&self) -> SessionData {
        self.session.lock().await.clone()
    }

    /// Build verified Gemini 4 Argon payload array (99 elements).
    /// Slot 71 holds the conversation identifier array when present.
    /// Slot 3 holds the previous response identifier when present.
    pub fn build_stream_generate_payload(
        prompt: &str,
        conv_id: Option<&str>,
        resp_id: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let mut inner: Vec<serde_json::Value> = vec![serde_json::Value::Null; 99];
        inner[0] = json!([prompt, 0, null, null, null, null, 0]);
        inner[1] = json!(["en"]);
        if let Some(rid) = resp_id {
            inner[3] = json!(rid);
        }
        inner[6] = json!([1]);
        inner[7] = json!(1);
        inner[10] = json!(1);
        inner[11] = json!(0);
        inner[17] = json!([[2]]); // Enforce Argon (Argon4p0 / 4 Argon mode)
        inner[18] = json!(0);
        inner[27] = json!(1);
        inner[30] = json!([4, 16, 19]);
        inner[41] = json!([1]);
        inner[49] = json!(0);
        inner[53] = json!(0);
        inner[61] = json!([]);
        inner[67] = json!(0);
        inner[68] = json!(2);
        if let Some(cid) = conv_id {
            inner[71] = json!([cid, null, null, null, null, null, null, null]);
        }
        inner[79] = json!(8);
        inner[80] = json!(1);
        inner[91] = json!(1);
        inner[96] = json!(0);
        inner[98] = json!(1);
        inner
    }

    /// Build the f.req form envelope string containing the JSON-serialized payload array.
    pub fn build_form_envelope(
        prompt: &str,
        conv_id: Option<&str>,
        resp_id: Option<&str>,
    ) -> String {
        let inner = Self::build_stream_generate_payload(prompt, conv_id, resp_id);
        let inner_json = serde_json::to_string(&inner).unwrap_or_default();
        json!([null, inner_json]).to_string()
    }

    /// Parse a single line from an upstream StreamGenerate chunk.
    /// Extracts (conversation_id, response_id, choice_id, candidate_text) if present.
    pub fn parse_wrb_fr_line(line: &str) -> Option<StreamChunkPayload> {
        let trimmed = line.trim();
        let stripped = trimmed.strip_prefix(")]}'").unwrap_or(trimmed).trim();
        if stripped.is_empty() || stripped.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }

        let val = serde_json::from_str::<serde_json::Value>(stripped).ok()?;

        let mut conv_id = None;
        let mut resp_id = None;
        let mut choice_id = None;
        let mut candidate_text = None;

        let mut entries = Vec::new();
        Self::collect_wrb_fr_entries(&val, &mut entries);

        for entry in entries {
            if let Some(payload_str) = entry.get(2).and_then(|v| v.as_str())
                && let Ok(inner_obj) = serde_json::from_str::<serde_json::Value>(payload_str)
            {
                if let Some(meta) = inner_obj.get(1).and_then(|m| m.as_array()) {
                    if let Some(cid) = meta.first().and_then(|s| s.as_str()) {
                        conv_id = Some(cid.to_string());
                    }
                    if let Some(rid) = meta.get(1).and_then(|s| s.as_str()) {
                        resp_id = Some(rid.to_string());
                    }
                }

                if let Some(candidates) = inner_obj.get(4).and_then(|c| c.as_array())
                    && let Some(first_candidate) = candidates.first()
                {
                    if let Some(chid) = first_candidate.get(0).and_then(|c| c.as_str()) {
                        choice_id = Some(chid.to_string());
                    }
                    if let Some(text_entry) = first_candidate.get(1).and_then(|t| t.as_array())
                        && let Some(content) = text_entry.first().and_then(|s| s.as_str())
                    {
                        candidate_text = Some(content.to_string());
                    }
                }
            }
        }

        if conv_id.is_some() || resp_id.is_some() || choice_id.is_some() || candidate_text.is_some()
        {
            Some(StreamChunkPayload {
                conversation_id: conv_id,
                response_id: resp_id,
                choice_id,
                candidate_text,
            })
        } else {
            None
        }
    }

    fn collect_wrb_fr_entries<'a>(
        val: &'a serde_json::Value,
        out: &mut Vec<&'a Vec<serde_json::Value>>,
    ) {
        if let Some(arr) = val.as_array() {
            if arr.first().and_then(|v| v.as_str()) == Some("wrb.fr") {
                out.push(arr);
            } else {
                for item in arr {
                    Self::collect_wrb_fr_entries(item, out);
                }
            }
        }
    }

    pub async fn send_prompt_stream(
        &self,
        prompt: &str,
        delta_tx: mpsc::Sender<Result<String, String>>,
    ) -> Result<String, String> {
        let (snlm0e, f_sid, bl, cookie, conv_id, resp_id) = {
            let s = self.session.lock().await;
            (
                s.snlm0e.clone(),
                s.f_sid.clone(),
                s.bl.clone(),
                s.cookie.clone(),
                s.conversation_id.clone(),
                s.response_id.clone(),
            )
        };

        let req_id = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 10_000_000) as u64;

        let url = format!(
            "https://gemini.google.com/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate?bl={}&f.sid={}&hl=en&_reqid={}&rt=c",
            bl, f_sid, req_id
        );

        let f_req = Self::build_form_envelope(prompt, conv_id.as_deref(), resp_id.as_deref());

        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded;charset=UTF-8"),
        );
        headers.insert("X-Same-Domain", HeaderValue::from_static("1"));
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("https://gemini.google.com"),
        );
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://gemini.google.com/"),
        );
        if let Ok(cookie_val) = HeaderValue::from_str(&cookie) {
            headers.insert(COOKIE, cookie_val);
        }

        info!("Sending native direct RPC to Gemini Web (StreamGenerate, Argon4p0)...");

        let response = match self
            .http
            .post(&url)
            .headers(headers)
            .form(&[("f.req", &f_req), ("at", &snlm0e)])
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = format!("Direct RPC request failed: {}", e);
                let _ = delta_tx.send(Err(err_msg.clone())).await;
                return Err(err_msg);
            }
        };

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!("Upstream Gemini returned HTTP {}: {}", status, error_text);
            let err_msg = format!("Gemini Web returned HTTP {}: {}", status, error_text);
            let _ = delta_tx.send(Err(err_msg.clone())).await;
            return Err(err_msg);
        }

        let mut stream = response.bytes_stream();
        let mut full_text = String::new();
        let mut line_buffer = String::new();

        let mut new_conv_id: Option<String> = None;
        let mut new_resp_id: Option<String> = None;
        let mut new_choice_id: Option<String> = None;

        while let Some(chunk_res) = stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(e) => {
                    let err_msg = format!("Stream read error: {}", e);
                    let _ = delta_tx.send(Err(err_msg.clone())).await;
                    return Err(err_msg);
                }
            };
            let chunk_str = String::from_utf8_lossy(&chunk);
            line_buffer.push_str(&chunk_str);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line: String = line_buffer[..newline_pos].trim().to_string();
                line_buffer.drain(..=newline_pos);
                Self::process_stream_line(
                    &line,
                    &mut full_text,
                    &mut new_conv_id,
                    &mut new_resp_id,
                    &mut new_choice_id,
                    &delta_tx,
                )
                .await;
            }
        }

        let remaining = line_buffer.trim().to_string();
        if !remaining.is_empty() {
            Self::process_stream_line(
                &remaining,
                &mut full_text,
                &mut new_conv_id,
                &mut new_resp_id,
                &mut new_choice_id,
                &delta_tx,
            )
            .await;
        }

        // Update session with conversation context if established
        if new_conv_id.is_some() || new_resp_id.is_some() || new_choice_id.is_some() {
            let mut s = self.session.lock().await;
            if let Some(cid) = new_conv_id {
                s.conversation_id = Some(cid);
            }
            if let Some(rid) = new_resp_id {
                s.response_id = Some(rid);
            }
            if let Some(chid) = new_choice_id {
                s.choice_id = Some(chid);
            }
            let _ = s.save();
        }

        Ok(full_text)
    }

    async fn process_stream_line(
        line: &str,
        full_text: &mut String,
        new_conv_id: &mut Option<String>,
        new_resp_id: &mut Option<String>,
        new_choice_id: &mut Option<String>,
        delta_tx: &mpsc::Sender<Result<String, String>>,
    ) {
        if let Some(parsed) = Self::parse_wrb_fr_line(line) {
            if let Some(cid) = parsed.conversation_id {
                *new_conv_id = Some(cid);
            }
            if let Some(rid) = parsed.response_id {
                *new_resp_id = Some(rid);
            }
            if let Some(chid) = parsed.choice_id {
                *new_choice_id = Some(chid);
            }
            if let Some(new_content) = parsed.candidate_text {
                let delta = if new_content.starts_with(full_text.as_str()) {
                    new_content[full_text.len()..].to_string()
                } else if full_text.is_empty() {
                    new_content.clone()
                } else {
                    let common_len = full_text
                        .chars()
                        .zip(new_content.chars())
                        .take_while(|(a, b)| a == b)
                        .map(|(c, _)| c.len_utf8())
                        .sum::<usize>();
                    if new_content.len() > common_len {
                        new_content[common_len..].to_string()
                    } else {
                        String::new()
                    }
                };
                if !delta.is_empty() {
                    let _ = delta_tx.send(Ok(delta)).await;
                    *full_text = new_content;
                }
            }
        }
    }
}
