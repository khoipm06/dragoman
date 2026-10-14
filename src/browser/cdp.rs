use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CdpTarget {
    pub id: String,
    pub title: String,
    pub r#type: String,
    pub url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    pub web_socket_debugger_url: Option<String>,
}

pub struct CdpClient {
    ws_write: Arc<
        Mutex<
            futures::stream::SplitSink<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
                Message,
            >,
        >,
    >,
    req_id: AtomicU64,
    pending_responses: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
}

impl CdpClient {
    pub async fn connect_to_gemini(cdp_port: u16) -> Result<Self, String> {
        let http_client = reqwest::Client::new();
        let list_url = format!("http://127.0.0.1:{}/json/list", cdp_port);

        let targets: Vec<CdpTarget> = match http_client.get(&list_url).send().await {
            Ok(resp) => resp.json().await.map_err(|e| e.to_string())?,
            Err(e) => {
                return Err(format!(
                    "Could not connect to CDP on port {}: {}",
                    cdp_port, e
                ));
            }
        };

        let gemini_target = targets
            .into_iter()
            .find(|t| t.url.contains("gemini.google.com"));

        let target = match gemini_target {
            Some(t) => t,
            None => {
                info!(
                    "No active Gemini tab found. Creating new tab for https://gemini.google.com/app..."
                );
                let new_url = format!(
                    "http://127.0.0.1:{}/json/new?https://gemini.google.com/app",
                    cdp_port
                );
                let created: CdpTarget = http_client
                    .put(&new_url)
                    .send()
                    .await
                    .map_err(|e| format!("Failed to open Gemini tab: {}", e))?
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse created tab: {}", e))?;
                tokio::time::sleep(Duration::from_secs(3)).await;
                created
            }
        };

        let ws_url = target
            .web_socket_debugger_url
            .ok_or_else(|| "Gemini tab has no webSocketDebuggerUrl".to_string())?;

        info!("Connecting to CDP WebSocket: {}", ws_url);
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("Failed to connect to CDP WebSocket: {}", e))?;

        let (write, mut read) = ws_stream.split();
        let write_mutex = Arc::new(Mutex::new(write));
        let pending = Arc::new(Mutex::new(
            HashMap::<u64, oneshot::Sender<serde_json::Value>>::new(),
        ));
        let pending_clone = Arc::clone(&pending);

        // Background reader that routes response messages by ID
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                if let Ok(Message::Text(text)) = msg
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(&text)
                    && let Some(id) = val.get("id").and_then(|i| i.as_u64())
                {
                    let mut map = pending_clone.lock().await;
                    if let Some(tx) = map.remove(&id) {
                        let _ = tx.send(val);
                    }
                }
            }
        });

        let client = Self {
            ws_write: write_mutex,
            req_id: AtomicU64::new(1),
            pending_responses: pending,
        };

        // Enable Runtime domain
        client.call_cdp("Runtime.enable", json!({})).await?;

        Ok(client)
    }

    pub async fn call_cdp(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.req_id.fetch_add(1, Ordering::SeqCst);
        let payload = json!({
            "id": id,
            "method": method,
            "params": params
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending_responses.lock().await;
            map.insert(id, tx);
        }

        {
            let mut write = self.ws_write.lock().await;
            write
                .send(Message::Text(payload.to_string().into()))
                .await
                .map_err(|e| format!("Failed to send CDP command: {}", e))?;
        }

        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err("CDP channel closed before response".to_string()),
            Err(_) => {
                let mut map = self.pending_responses.lock().await;
                map.remove(&id);
                Err("CDP call timed out".to_string())
            }
        }
    }

    pub async fn evaluate_js(&self, expression: &str) -> Result<serde_json::Value, String> {
        let resp = self
            .call_cdp(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true
                }),
            )
            .await?;

        if let Some(result) = resp.get("result").and_then(|r| r.get("result"))
            && let Some(val) = result.get("value")
        {
            return Ok(val.clone());
        }

        Ok(serde_json::Value::Null)
    }

    #[allow(dead_code)]
    pub async fn ensure_model_selected(&self, _target_model: &str) -> Result<String, String> {
        let js = r#"
            (async function() {
                const pickerBtn = document.querySelector('button[aria-label*="mode picker"], button[aria-label*="model picker"]') ||
                                  Array.from(document.querySelectorAll('button')).find(b => /flash|pro|argon/i.test(b.innerText || ''));
                if (!pickerBtn) return "Unknown";

                const currentText = pickerBtn.innerText.trim();
                if (/argon/i.test(currentText)) {
                    return currentText;
                }

                // If not Argon, open menu and select 4 Argon (Argon4p0)
                pickerBtn.click();
                await new Promise(r => setTimeout(r, 400));

                const menuItems = Array.from(document.querySelectorAll('[role="menuitem"], .mat-mdc-menu-item'));
                const argonItem = menuItems.find(m => /4 Argon|Argon/i.test(m.innerText || ''));
                if (argonItem) {
                    argonItem.click();
                    await new Promise(r => setTimeout(r, 600));
                    return "Argon (Argon4p0)";
                }

                return currentText;
            })();
        "#;
        let val = self.evaluate_js(js).await?;
        let active = val.as_str().unwrap_or("Unknown").to_string();
        info!("Active Gemini Web model: {}", active);
        Ok(active)
    }

    #[allow(dead_code)]
    pub async fn check_model_selected(&self, target_model: &str) -> Result<String, String> {
        self.ensure_model_selected(target_model).await
    }

    #[allow(dead_code)]
    pub async fn submit_prompt_and_stream(
        &self,
        prompt: &str,
        delta_tx: mpsc::Sender<String>,
    ) -> Result<String, String> {
        // Enforce Gemini 4 Argon selection before submitting prompt
        let _ = self.ensure_model_selected("Argon").await;

        let escaped_prompt = serde_json::to_string(prompt).unwrap_or_else(|_| "\"\"".to_string());

        // 0. Get current response count to isolate new turn
        let count_js = r#"
            (function() {
                return document.querySelectorAll('.model-response-text, message-content, [data-test-id="model-turn"]').length;
            })();
        "#;
        let count_val = self
            .evaluate_js(count_js)
            .await
            .unwrap_or(serde_json::Value::from(0));
        let count_before = count_val.as_u64().unwrap_or(0);

        // 1. Inject prompt and trigger send in Gemini Web
        let injection_js = format!(
            r#"
            (async function() {{
                const inputEl = document.querySelector('rich-textarea div[contenteditable="true"]') ||
                                document.querySelector('div[role="textbox"]') ||
                                document.querySelector('textarea');
                
                if (!inputEl) {{
                    throw new Error("Gemini input box not found in page. Ensure you are logged into gemini.google.com");
                }}

                inputEl.focus();
                inputEl.innerText = {prompt_json};
                inputEl.dispatchEvent(new Event('input', {{ bubbles: true }}));
                inputEl.dispatchEvent(new Event('change', {{ bubbles: true }}));

                await new Promise(r => setTimeout(r, 400));

                const sendBtn = document.querySelector('button[aria-label*="Send"], button[jsname*="send"], button.send-button');
                if (sendBtn && !sendBtn.disabled) {{
                    sendBtn.click();
                    return "SENT_CLICKED";
                }}

                inputEl.dispatchEvent(new KeyboardEvent('keydown', {{
                    key: 'Enter',
                    code: 'Enter',
                    keyCode: 13,
                    which: 13,
                    bubbles: true
                }}));
                return "SENT_ENTER";
            }})();
            "#,
            prompt_json = escaped_prompt
        );

        info!("Submitting prompt to Gemini Web UI...");
        self.evaluate_js(&injection_js).await?;

        // 2. Poll for generated response deltas
        let mut previous_length = 0;
        let mut full_output = String::new();
        let mut idle_ticks = 0;
        let poll_interval = Duration::from_millis(250);

        let poll_script = format!(
            r#"
            (function() {{
                const responses = document.querySelectorAll('.model-response-text, message-content, [data-test-id="model-turn"]');
                if (responses.length <= {count_before}) {{
                    return {{ text: "", isDone: false, waiting: true }};
                }}

                const latest = responses[responses.length - 1];
                const text = latest.innerText || latest.textContent || "";
                const stopBtn = document.querySelector('button[aria-label*="Stop"], button.stop-button');
                const isGenerating = !!stopBtn;

                return {{
                    text: text,
                    isDone: !isGenerating,
                    waiting: false
                }};
            }})();
            "#,
            count_before = count_before
        );

        for _ in 0..1200 {
            // Max 5 minutes for deep reasoning (Argon)
            tokio::time::sleep(poll_interval).await;
            let val = match self.evaluate_js(&poll_script).await {
                Ok(v) => v,
                Err(e) => {
                    warn!("Error polling response: {}", e);
                    continue;
                }
            };

            let current_text = val.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let is_done = val.get("isDone").and_then(|d| d.as_bool()).unwrap_or(false);

            if current_text.len() > previous_length {
                let delta = &current_text[previous_length..];
                let _ = delta_tx.send(delta.to_string()).await;
                previous_length = current_text.len();
                full_output = current_text.to_string();
                idle_ticks = 0;
            } else if is_done && !full_output.is_empty() {
                idle_ticks += 1;
                if idle_ticks >= 3 {
                    // confirmed finished
                    break;
                }
            } else if !full_output.is_empty() {
                idle_ticks += 1;
                if idle_ticks > 40 {
                    // 10 seconds without new text and done
                    break;
                }
            }
        }

        Ok(full_output)
    }
}
