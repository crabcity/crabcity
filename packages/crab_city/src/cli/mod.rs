pub mod attach;
pub mod daemon;
pub mod picker;
pub mod terminal;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde::Deserialize;
use tokio_tungstenite::tungstenite;

use crate::config::CrabCityConfig;
use daemon::DaemonInfo;
use picker::{PickerEvent, PickerResult};

/// Default command: ensure daemon, show picker if instances exist, else create new.
/// After detaching from a session, returns to the picker.
pub async fn default_command(config: &CrabCityConfig) -> Result<()> {
    let daemon = daemon::ensure_daemon(config).await?;

    // First run: if no instances at all, create one directly
    let instances = fetch_instances(&daemon).await?;
    if instances.is_empty() {
        let cwd = std::env::current_dir()
            .context("Failed to get current directory")?
            .to_string_lossy()
            .to_string();
        let instance = create_instance(&daemon, None, Some(&cwd)).await?;
        attach::attach(&daemon, &instance.id).await?;
    }

    session_loop(&daemon).await
}

/// Attach to an existing instance (by name, ID, or prefix). No target: show picker.
/// After detaching from a session, returns to the picker.
pub async fn attach_command(config: &CrabCityConfig, target: Option<String>) -> Result<()> {
    let daemon = daemon::ensure_daemon(config).await?;

    if let Some(t) = target {
        let instance_id = resolve_instance(&daemon, &t).await?;
        return attach::attach(&daemon, &instance_id).await;
    }

    session_loop(&daemon).await
}

/// Picker → attach → detach → picker loop. Exits on Quit.
async fn session_loop(daemon: &DaemonInfo) -> Result<()> {
    loop {
        let instances = fetch_instances(daemon).await?;

        match run_live_picker(daemon, instances).await? {
            PickerResult::Attach(id) => {
                attach::attach(daemon, &id).await?;
            }
            PickerResult::NewInstance => {
                let cwd = std::env::current_dir()
                    .context("Failed to get current directory")?
                    .to_string_lossy()
                    .to_string();
                let instance = create_instance(daemon, None, Some(&cwd)).await?;
                attach::attach(daemon, &instance.id).await?;
            }
            PickerResult::Quit => return Ok(()),
        }
        // After detach, loop back to picker
    }
}

/// Connect to the mux WebSocket for live updates and run the picker.
async fn run_live_picker(
    daemon: &DaemonInfo,
    instances: Vec<InstanceInfo>,
) -> Result<PickerResult> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Best-effort WS connection for live updates; picker works fine without it
    if let Ok((ws, _)) = tokio_tungstenite::connect_async(daemon.mux_ws_url()).await {
        let (_, mut ws_read) = ws.split();
        tokio::spawn(async move {
            while let Some(Ok(tungstenite::Message::Text(text))) = ws_read.next().await {
                if let Ok(ev) = serde_json::from_str::<WsLifecycleEvent>(&text) {
                    let picker_ev = match ev {
                        WsLifecycleEvent::InstanceCreated { instance } => {
                            PickerEvent::Created(instance)
                        }
                        WsLifecycleEvent::InstanceStopped { instance_id } => {
                            PickerEvent::Stopped(instance_id)
                        }
                    };
                    if tx.send(picker_ev).is_err() {
                        break; // picker dropped the receiver
                    }
                }
                // Silently ignore all other message types
            }
        });
    }

    picker::run_picker(instances, rx)
}

/// Subset of server messages we care about in the CLI picker.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsLifecycleEvent {
    InstanceCreated { instance: InstanceInfo },
    InstanceStopped { instance_id: String },
}

/// List running instances.
pub async fn list_command(config: &CrabCityConfig, json: bool) -> Result<()> {
    let daemon = daemon::ensure_daemon(config).await?;

    let instances = fetch_instances(&daemon).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&instances)?);
    } else if instances.is_empty() {
        println!("No running instances.");
    } else {
        // Table header
        println!(
            "{:<38} {:<20} {:<8} {}",
            "ID", "NAME", "STATUS", "WORKING DIR"
        );
        println!("{}", "-".repeat(100));
        for inst in &instances {
            let status = if inst.running { "running" } else { "stopped" };
            // Show short ID (first 8 chars)
            let short_id = if inst.id.len() > 8 {
                &inst.id[..8]
            } else {
                &inst.id
            };
            println!(
                "{:<38} {:<20} {:<8} {}",
                short_id, inst.name, status, inst.working_dir
            );
        }
        println!("\n{} instance(s)", instances.len());
    }

    Ok(())
}

#[derive(Deserialize, serde::Serialize, Clone)]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub working_dir: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct CreateInstanceResponse {
    id: String,
    name: String,
}

async fn fetch_instances(daemon: &DaemonInfo) -> Result<Vec<InstanceInfo>> {
    let url = format!("{}/api/instances", daemon.base_url());
    let resp = reqwest::get(&url)
        .await
        .context("Failed to list instances")?;
    resp.json().await.context("Failed to parse instance list")
}

async fn create_instance(
    daemon: &DaemonInfo,
    name: Option<&str>,
    working_dir: Option<&str>,
) -> Result<CreateInstanceResponse> {
    let url = format!("{}/api/instances", daemon.base_url());
    let body = serde_json::json!({
        "name": name,
        "working_dir": working_dir,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to create instance")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create instance: {} {}", status, text);
    }

    resp.json()
        .await
        .context("Failed to parse create instance response")
}

async fn resolve_instance(daemon: &DaemonInfo, target: &str) -> Result<String> {
    let instances = fetch_instances(daemon).await?;

    if instances.is_empty() {
        anyhow::bail!("No running instances. Use `crab` to create one.");
    }

    // Try exact ID match
    if let Some(inst) = instances.iter().find(|i| i.id == target) {
        return Ok(inst.id.clone());
    }
    // Try name match
    if let Some(inst) = instances.iter().find(|i| i.name == target) {
        return Ok(inst.id.clone());
    }
    // Try ID prefix match
    let prefix_matches: Vec<_> = instances
        .iter()
        .filter(|i| i.id.starts_with(target))
        .collect();
    match prefix_matches.len() {
        0 => anyhow::bail!("No instance found matching '{}'", target),
        1 => Ok(prefix_matches[0].id.clone()),
        n => anyhow::bail!(
            "Ambiguous: '{}' matches {} instances. Be more specific.",
            target,
            n
        ),
    }
}
