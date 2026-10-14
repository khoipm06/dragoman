use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER, USER_AGENT};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info};

use crate::session::SessionData;

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

    pub async fn send_prompt_stream(
        &self,
        prompt: &str,
        delta_tx: mpsc::Sender<String>,
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

        // Build verified Gemini 4 Argon payload array (99 elements)
        let mut inner: Vec<serde_json::Value> = vec![serde_json::Value::Null; 99];
        inner[0] = json!([prompt, 0, null, null, null, null, 0]);
        inner[1] = json!(["en"]);
        if let Some(ref rid) = resp_id {
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
        if let Some(ref cid) = conv_id {
            inner[71] = json!([cid, null, null, null, null, null, null, null]);
        }
        inner[79] = json!(8);
        inner[80] = json!(1);
        inner[91] = json!(1);
        inner[96] = json!(0);
        inner[98] = json!(1);

        let inner_json = serde_json::to_string(&inner).unwrap_or_default();
        let f_req = json!([null, inner_json]).to_string();

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

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .form(&[("f.req", &f_req), ("at", &snlm0e)])
            .send()
            .await
            .map_err(|e| format!("Direct RPC request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!("Upstream Gemini returned HTTP {}: {}", status, error_text);
            return Err(format!(
                "Gemini Web returned HTTP {}: {}",
                status, error_text
            ));
        }

        let mut stream = response.bytes_stream();
        let mut full_text = String::new();
        let mut line_buffer = String::new();

        let mut new_conv_id: Option<String> = None;
        let mut new_resp_id: Option<String> = None;

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.map_err(|e| format!("Stream read error: {}", e))?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            line_buffer.push_str(&chunk_str);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].trim().to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                if line.is_empty()
                    || line.starts_with(")]}'")
                    || line.chars().all(|c| c.is_ascii_digit())
                {
                    continue;
                }

                // Parse batch array
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                    && let Some(arr) = val.as_array()
                {
                    for item in arr {
                        if let Some(entry) = item.as_array()
                            && entry.first().and_then(|v| v.as_str()) == Some("wrb.fr")
                            && let Some(payload_str) = entry.get(2).and_then(|v| v.as_str())
                            && let Ok(inner_obj) =
                                serde_json::from_str::<serde_json::Value>(payload_str)
                        {
                            // Extract conversation & response IDs if present
                            if let Some(meta) = inner_obj.get(1).and_then(|m| m.as_array()) {
                                if let Some(cid) = meta.first().and_then(|s| s.as_str()) {
                                    new_conv_id = Some(cid.to_string());
                                }
                                if let Some(rid) = meta.get(1).and_then(|s| s.as_str()) {
                                    new_resp_id = Some(rid.to_string());
                                }
                            }

                            Self::extract_and_emit_delta(&inner_obj, &mut full_text, &delta_tx)
                                .await;
                        }
                    }
                }
            }
        }

        // Update session with conversation context if established
        if new_conv_id.is_some() || new_resp_id.is_some() {
            let mut s = self.session.lock().await;
            if let Some(cid) = new_conv_id {
                s.conversation_id = Some(cid);
            }
            if let Some(rid) = new_resp_id {
                s.response_id = Some(rid);
            }
            let _ = s.save();
        }

        Ok(full_text)
    }

    async fn extract_and_emit_delta(
        val: &serde_json::Value,
        full_text: &mut String,
        delta_tx: &mpsc::Sender<String>,
    ) {
        // In StreamGenerate payload, candidate text is at val[4][0][1][0]
        if let Some(candidates) = val.get(4).and_then(|c| c.as_array())
            && let Some(first_candidate) = candidates.first()
            && let Some(text_entry) = first_candidate.get(1).and_then(|t| t.as_array())
            && let Some(new_content) = text_entry.first().and_then(|s| s.as_str())
            && new_content.len() > full_text.len()
        {
            let delta = &new_content[full_text.len()..];
            let _ = delta_tx.send(delta.to_string()).await;
            *full_text = new_content.to_string();
        }
    }
}
