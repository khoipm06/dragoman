use clap::{Parser, Subcommand};
use dragoman::browser::{BrowserLauncher, CdpClient};
use dragoman::proxy::server::{AppState, create_router};
use dragoman::session::SessionData;
use dragoman::web::GeminiWebClient;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser, Debug)]
#[command(name = "dragoman")]
#[command(author = "Khoi Pham Minh <khoipm06@gmail.com>")]
#[command(version = "0.1.0")]
#[command(about = "High-performance native OpenAI-compatible proxy for Gemini Web (Gemini 4 Argon)", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the standalone OpenAI-compatible proxy server (no browser needed)
    Serve {
        /// HTTP listen port for external coding agents
        #[arg(short, long, default_value_t = 8080)]
        port: u16,

        /// Target model identifier reported to coding agents
        #[arg(short, long, default_value = "gemini-4-argon")]
        model: String,
    },
    /// Perform one-time browser login to extract and save session credentials
    Login {
        /// Chrome DevTools Protocol (CDP) port used during login
        #[arg(long, default_value_t = 9222)]
        cdp_port: u16,
    },
    /// Test direct native HTTP RPC without browser
    TestRpc {
        #[arg(default_value = "Say hello and confirm you are Gemini 4 Argon in 1 sentence.")]
        prompt: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    fmt().with_env_filter(filter).init();

    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Serve {
        port: 8080,
        model: "gemini-4-argon".to_string(),
    }) {
        Commands::Serve { port, model } => {
            println!("=======================================================");
            println!(" Dragoman - Native Gemini Web OpenAI Bridge");
            println!(" Target Model: {}", model);
            println!(" Mode: Standalone HTTP/2 Direct RPC (Zero Browser Runtime)");
            println!(
                " Proxy Endpoint: http://127.0.0.1:{}/v1/chat/completions",
                port
            );
            println!("=======================================================\n");

            let session = match SessionData::load() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    eprintln!(
                        "Please run 'dragoman login' first to authenticate with your Google AI Pro account."
                    );
                    std::process::exit(1);
                }
            };

            let web_client = Arc::new(GeminiWebClient::new(session));
            let app_state = Arc::new(AppState {
                web_client,
                default_model: model,
            });

            let router = create_router(app_state);
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            info!("Dragoman listening on http://{}", addr);

            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(listener, router).await?;
        }
        Commands::Login { cdp_port } => {
            println!("=== Dragoman Initial Login & Credential Extraction ===");
            println!(
                "Ensuring Chrome is running with remote debugging on port {}...",
                cdp_port
            );

            BrowserLauncher::ensure_chrome_running(cdp_port, None)
                .await
                .map_err(|e| format!("Failed to launch browser: {}", e))?;

            println!("Connecting to Chrome DevTools to extract session tokens...");
            let client = CdpClient::connect_to_gemini(cdp_port).await?;

            println!("Extracting session tokens (SNlM0e, FdrFJe, cfb2h) and cookies...");
            let snlm0e = client
                .evaluate_js("window.WIZ_global_data?.SNlM0e || ''")
                .await?
                .as_str()
                .unwrap_or("")
                .to_string();
            let f_sid = client
                .evaluate_js("window.WIZ_global_data?.FdrFJe || ''")
                .await?
                .as_str()
                .unwrap_or("")
                .to_string();
            let bl = client
                .evaluate_js("window.WIZ_global_data?.cfb2h || ''")
                .await?
                .as_str()
                .unwrap_or("")
                .to_string();

            if snlm0e.is_empty() {
                eprintln!(
                    "\nWarning: Could not read SNlM0e. Please ensure you are logged in to https://gemini.google.com in the browser window."
                );
                return Ok(());
            }

            // Extract cookies via CDP
            let cookies_val = client
                .call_cdp(
                    "Network.getCookies",
                    serde_json::json!({
                        "urls": ["https://gemini.google.com"]
                    }),
                )
                .await?;

            let mut cookie_header = String::new();
            if let Some(cookies) = cookies_val
                .get("result")
                .and_then(|r| r.get("cookies"))
                .and_then(|c| c.as_array())
            {
                let parts: Vec<String> = cookies
                    .iter()
                    .filter_map(|c| {
                        let name = c.get("name")?.as_str()?;
                        let value = c.get("value")?.as_str()?;
                        Some(format!("{}={}", name, value))
                    })
                    .collect();
                cookie_header = parts.join("; ");
            }

            let session = SessionData {
                snlm0e,
                f_sid,
                bl,
                cookie: cookie_header,
                model: "Argon4p0".to_string(),
                conversation_id: None,
                response_id: None,
                choice_id: None,
            };

            session.save()?;
            println!("\nSUCCESS: Saved authentication session to ~/.config/dragoman/session.json!");
            println!("You can now close Chrome completely.");
            println!("Run 'dragoman serve' to launch the proxy without any browser running.");
        }
        Commands::TestRpc { prompt } => {
            let session =
                SessionData::load().map_err(|e| format!("Could not load session: {}", e))?;
            println!("Loaded session: bl={}, model={}", session.bl, session.model);
            let client = GeminiWebClient::new(session);
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);

            println!("Sending prompt: \"{}\"", prompt);
            tokio::spawn(async move {
                while let Some(delta) = rx.recv().await {
                    print!("{}", delta);
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
            });

            match client.send_prompt_stream(&prompt, tx).await {
                Ok(full) => {
                    println!("\n\n--- Direct RPC Completed successfully ---");
                    println!("Total chars received: {}", full.len());
                }
                Err(e) => {
                    eprintln!("\nDirect RPC error: {}", e);
                }
            }
        }
    }

    Ok(())
}
