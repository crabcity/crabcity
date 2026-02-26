//! `crab switch` — list and switch between local and remote Crab City contexts.

use anyhow::Result;
use serde::Deserialize;

use super::daemon::DaemonInfo;

#[derive(Deserialize)]
struct RemoteEntry {
    host_node_id: String,
    host_name: String,
    granted_access: String,
    status: String,
}

/// `crab switch` with no arguments: list available contexts.
/// `crab switch <name>`: connect to the named remote.
/// `crab switch home`: switch back to local context.
pub async fn switch_command(daemon: &DaemonInfo, target: Option<&str>) -> Result<()> {
    let remotes = fetch_remotes(daemon).await?;

    match target {
        None => {
            // List all available contexts
            println!("  * local (home)");
            if remotes.is_empty() {
                println!("\n  No remote connections. Use `crab connect <token>` to join one.");
            } else {
                for r in &remotes {
                    let status_mark = match r.status.as_str() {
                        "connected" => "+",
                        "reconnecting" => "~",
                        _ => "-",
                    };
                    println!(
                        "  {} {} ({}, {})",
                        status_mark, r.host_name, r.granted_access, r.status
                    );
                }
                println!();
                println!("  Switch with: crab switch '<name>'");
            }
            Ok(())
        }
        Some("home" | "local") => {
            // Nothing to do — CLI always starts in local context.
            // The TUI picker handles context switching at runtime.
            println!("Switched to local context.");
            Ok(())
        }
        Some(name) => {
            // Find matching remote by name (case-insensitive prefix match)
            let matches: Vec<&RemoteEntry> = remotes
                .iter()
                .filter(|r| {
                    r.host_name.eq_ignore_ascii_case(name)
                        || r.host_name
                            .to_ascii_lowercase()
                            .starts_with(&name.to_ascii_lowercase())
                })
                .collect();

            match matches.len() {
                0 => {
                    eprintln!(
                        "No remote matching '{}'. Run `crab switch` to see available.",
                        name
                    );
                    std::process::exit(1);
                }
                1 => {
                    let remote = matches[0];
                    if remote.status == "connected" {
                        println!("Already connected to {}.", remote.host_name);
                    } else {
                        // Trigger connect via daemon API
                        eprintln!("Connecting to {}...", remote.host_name);
                        connect_remote(daemon, &remote.host_node_id).await?;
                        println!("Connected to {}.", remote.host_name);
                    }
                    Ok(())
                }
                n => {
                    eprintln!(
                        "Ambiguous: '{}' matches {} remotes. Be more specific:",
                        name, n
                    );
                    for m in &matches {
                        eprintln!("  {}", m.host_name);
                    }
                    std::process::exit(1);
                }
            }
        }
    }
}

async fn fetch_remotes(daemon: &DaemonInfo) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/api/remotes", daemon.base_url());
    let resp = reqwest::get(&url).await?;
    let remotes: Vec<RemoteEntry> = resp.json().await?;
    Ok(remotes)
}

async fn connect_remote(daemon: &DaemonInfo, host_node_id: &str) -> Result<()> {
    let url = format!("{}/api/remotes/connect", daemon.base_url());
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "host_node_id": host_node_id }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(err) = body.get("error") {
        anyhow::bail!("Connection failed: {}", err);
    }
    Ok(())
}
