use crate::daemon::status_path;
use crate::state::{self, Status};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.wassimk.calendar-autoblock";

fn home() -> PathBuf {
    dirs::home_dir().expect("no home directory")
}

fn plist_path() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn log_dir() -> PathBuf {
    home().join("Library/Logs/calendar-autoblock")
}

fn current_uid() -> String {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .expect("failed to run id -u");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn service_target() -> String {
    format!("gui/{}/{LABEL}", current_uid())
}

pub fn generate_plist(binary_path: &str) -> String {
    let logs = log_dir();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary_path}</string>
        <string>start</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>"#,
        stdout = logs.join("stdout.log").display(),
        stderr = logs.join("stderr.log").display(),
    )
}

pub fn install_launchagent(force: bool) -> Result<()> {
    let plist = plist_path();
    if plist.exists() && !force {
        anyhow::bail!(
            "LaunchAgent already installed at {}\nUse --force to overwrite.",
            plist.display()
        );
    }
    let binary_path = std::env::current_exe()
        .context("Failed to determine binary path")?
        .to_string_lossy()
        .to_string();
    if plist.exists() {
        let _ = Command::new("launchctl")
            .args(["bootout", &service_target()])
            .output();
    }
    std::fs::create_dir_all(log_dir()).context("Failed to create log directory")?;
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&plist, generate_plist(&binary_path))
        .with_context(|| format!("Failed to write plist to {}", plist.display()))?;
    let output = Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{}", current_uid()),
            &plist.to_string_lossy(),
        ])
        .output()
        .context("Failed to run launchctl bootstrap")?;
    if !output.status.success() {
        anyhow::bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    println!("LaunchAgent installed and loaded.");
    println!("  Plist: {}", plist.display());
    println!("  Logs:  {}", log_dir().display());
    Ok(())
}

pub fn uninstall_launchagent() -> Result<()> {
    let _ = Command::new("launchctl")
        .args(["bootout", &service_target()])
        .output();
    let plist = plist_path();
    if plist.exists() {
        std::fs::remove_file(&plist)
            .with_context(|| format!("Failed to remove {}", plist.display()))?;
        println!("LaunchAgent uninstalled.");
        println!(
            "Copies already on your work calendar are left in place; run `calendar-autoblock cleanup` to remove them."
        );
    } else {
        println!("LaunchAgent was not installed.");
    }
    Ok(())
}

pub fn stop_launchagent() -> Result<()> {
    let output = Command::new("launchctl")
        .args(["bootout", &service_target()])
        .output()
        .context("Failed to run launchctl bootout")?;
    if output.status.success() {
        println!("Service stopped.");
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such process") || stderr.contains("Could not find service") {
        println!("Service is not running.");
        Ok(())
    } else {
        anyhow::bail!("Failed to stop service: {}", stderr.trim())
    }
}

pub fn restart_launchagent() -> Result<()> {
    let output = Command::new("launchctl")
        .args(["kickstart", "-k", &service_target()])
        .output()
        .context("Failed to run launchctl kickstart")?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to restart service: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    println!("Service restarted.");
    Ok(())
}

pub fn is_running() -> bool {
    Command::new("launchctl")
        .args(["list", LABEL])
        .output()
        .is_ok_and(|o| o.status.success())
}

pub fn show_status() -> Result<()> {
    let running = is_running();
    let status: Status = state::load_json(&status_path())?;
    let yes_no = |v: bool| if v { "yes" } else { "no" };
    let local = |t: chrono::DateTime<chrono::Utc>| {
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    };

    println!("Service:    {LABEL}");
    println!("Installed:  {}", yes_no(plist_path().exists()));
    println!("Running:    {}", yes_no(running));
    println!(
        "Last pass:  {}",
        status
            .last_pass
            .map(local)
            .unwrap_or_else(|| "never".into())
    );
    if let Some(since) = status.offline_since {
        println!("Offline since {}", local(since));
    }
    let c = status.last_counts;
    println!(
        "Last counts: {} created, {} patched, {} deleted",
        c.created, c.patched, c.deleted
    );
    if !status.last_success.is_empty() {
        println!();
        println!("Last successful read:");
        for (cal, t) in &status.last_success {
            println!("  {cal}: {}", local(*t));
        }
    }
    for (account, msg) in &status.auth_errors {
        println!();
        println!("Auth error ({account}): {msg}");
    }
    for (cal, msg) in &status.calendar_errors {
        println!("Calendar {cal}: {msg}");
    }
    if let Some(e) = &status.last_error {
        println!("Last error: {e}");
    }
    println!();
    println!("Paths:");
    println!("  Plist:  {}", plist_path().display());
    println!("  Config: {}", crate::config::config_path().display());
    println!("  State:  {}", state::state_dir().display());
    println!("  Logs:   {}", log_dir().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_runs_start_with_keepalive() {
        let plist = generate_plist("/opt/homebrew/bin/calendar-autoblock");
        assert!(plist.contains("<string>/opt/homebrew/bin/calendar-autoblock</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<key>KeepAlive</key>\n    <true/>"));
    }

    #[test]
    fn plist_uses_calendar_autoblock_label_and_logs() {
        let plist = generate_plist("/bin/x");
        assert!(plist.contains("<string>com.wassimk.calendar-autoblock</string>"));
        assert!(plist.contains("Library/Logs/calendar-autoblock/stdout.log"));
        assert!(plist.contains("Library/Logs/calendar-autoblock/stderr.log"));
        assert!(!plist.contains("SHELL"));
    }
}
