//! `crab invite` CLI subcommands: create, list, revoke.

use anyhow::Result;

use crate::cli::daemon::{DaemonError, DaemonInfo};

/// Create an invite via POST /api/invites and display the token + QR code.
pub async fn invite_create_command(
    daemon: &DaemonInfo,
    capability: &str,
    max_uses: u32,
    expires_in_secs: Option<u64>,
    label: Option<&str>,
) -> Result<(), DaemonError> {
    let url = format!("{}/api/invites", daemon.base_url());
    let body = serde_json::json!({
        "capability": capability,
        "max_uses": max_uses,
        "expires_in_secs": expires_in_secs,
        "label": label,
    });

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(DaemonError::from_reqwest)?;

    let json: serde_json::Value = resp.json().await.map_err(DaemonError::from_reqwest)?;

    if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
        eprintln!("Error: {}", error);
        return Ok(());
    }

    let token = json["token"].as_str().unwrap_or("???");
    let capability = json["capability"].as_str().unwrap_or("???");
    let max_uses = json["max_uses"].as_u64().unwrap_or(0);
    let expires_at = json["expires_at"].as_str();
    let instance_name = json["instance_name"].as_str();

    eprintln!();
    if let Some(name) = instance_name {
        eprintln!("  Invite for {}", name);
    } else {
        eprintln!("  Invite created");
    }
    if let Some(lbl) = label {
        eprintln!("  Label:      {}", lbl);
    }
    eprintln!("  Access:     {}", capability);
    eprintln!(
        "  Max uses:   {}",
        if max_uses == 0 {
            "unlimited".to_string()
        } else {
            max_uses.to_string()
        }
    );
    if let Some(exp) = expires_at {
        eprintln!("  Expires:    {}", exp);
    }
    eprintln!();

    // Print the token (stdout for piping)
    println!("{}", token);

    // Try to copy to clipboard (best-effort, silent failure)
    if try_copy_to_clipboard(token) {
        eprintln!("  (copied to clipboard)");
    }

    eprintln!();

    // Print QR code using Unicode half-blocks
    print_qr(token);

    eprintln!();
    eprintln!("  Connect with:");
    eprintln!("    crab connect {}", token);

    Ok(())
}

/// List all invites via GET /api/invites.
pub async fn invite_list_command(daemon: &DaemonInfo) -> Result<(), DaemonError> {
    let url = format!("{}/api/invites", daemon.base_url());
    let resp = reqwest::get(&url)
        .await
        .map_err(DaemonError::from_reqwest)?;

    let json: serde_json::Value = resp.json().await.map_err(DaemonError::from_reqwest)?;

    if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
        eprintln!("Error: {}", error);
        return Ok(());
    }

    let invites = json["invites"].as_array();
    match invites {
        Some(invites) if invites.is_empty() => {
            println!("No invites.");
        }
        Some(invites) => {
            println!(
                "{:<34} {:<12} {:<8} {:<10} {}",
                "NONCE", "CAPABILITY", "USES", "STATE", "LABEL"
            );
            println!("{}", "-".repeat(80));
            for inv in invites {
                let nonce = inv["nonce"].as_str().unwrap_or("???");
                let cap = inv["capability"].as_str().unwrap_or("???");
                let uses = format!(
                    "{}/{}",
                    inv["use_count"].as_u64().unwrap_or(0),
                    inv["max_uses"].as_u64().unwrap_or(0)
                );
                let state = inv["state"].as_str().unwrap_or("???");
                let label = inv["label"].as_str().unwrap_or("");
                // Truncate nonce for display
                let nonce_short = if nonce.len() > 32 {
                    &nonce[..32]
                } else {
                    nonce
                };
                println!(
                    "{:<34} {:<12} {:<8} {:<10} {}",
                    nonce_short, cap, uses, state, label
                );
            }
            println!("\n{} invite(s)", invites.len());
        }
        None => {
            println!("No invites.");
        }
    }

    Ok(())
}

/// Revoke an invite via DELETE /api/invites/{nonce}.
pub async fn invite_revoke_command(daemon: &DaemonInfo, nonce: &str) -> Result<(), DaemonError> {
    let url = format!("{}/api/invites/{}", daemon.base_url(), nonce);
    let resp = reqwest::Client::new()
        .delete(&url)
        .send()
        .await
        .map_err(DaemonError::from_reqwest)?;

    let json: serde_json::Value = resp.json().await.map_err(DaemonError::from_reqwest)?;

    if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
        eprintln!("Error: {}", error);
    } else {
        eprintln!("Invite revoked: {}", nonce);
    }

    Ok(())
}

/// Render a QR code to the terminal using Unicode half-blocks.
///
/// Uses ▀▄█ and space to represent 2 rows per line (each char is 2 modules tall).
fn print_qr(data: &str) {
    use qrcode::QrCode;

    let code = match QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("(QR code generation failed: {})", e);
            return;
        }
    };

    let image = code
        .render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    eprintln!("{}", image);
}

/// Best-effort clipboard copy. Returns true if successful.
fn try_copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().is_ok_and(|s| s.success());
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        // Try wl-copy (Wayland) first, then xclip (X11)
        for cmd in &["wl-copy", "xclip"] {
            let args: &[&str] = if *cmd == "xclip" {
                &["-selection", "clipboard"]
            } else {
                &[]
            };
            if let Ok(mut child) = Command::new(cmd)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().is_ok_and(|s| s.success()) {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = text;
        false
    }
}
