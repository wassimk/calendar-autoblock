#![allow(dead_code)] // modules land before main wires them up; removed in Task 12

mod api;
mod config;
mod eligibility;
mod event;
mod keychain;
mod oauth;
mod reconcile;
mod state;
mod sync;
mod window;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "calendar-autoblock",
    version,
    about = "Block time on your work Google Calendar for personal appointments"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Guided first run: sign in, pick calendars, set work hours
    Setup,
    /// List, add, or remove source calendars
    Calendars {
        #[command(subcommand)]
        command: CalendarsCommand,
    },
    /// Sign in to an account, or check an existing sign-in
    Auth {
        account: String,
        /// Refresh a token from the Keychain instead of opening the browser
        #[arg(long)]
        check: bool,
    },
    /// Run one sync pass
    Sync {
        /// Print what would change without changing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete every future copy this tool created
    Cleanup {
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Run the sync loop in the foreground
    Start {
        #[arg(long)]
        verbose: bool,
    },
    /// Install the LaunchAgent for automatic startup
    Install {
        #[arg(long)]
        force: bool,
    },
    /// Uninstall the LaunchAgent
    Uninstall,
    /// Stop the background service
    Stop,
    /// Restart the background service
    Restart,
    /// Show running state, last sync, auth health, and log paths
    Status,
}

#[derive(Subcommand)]
enum CalendarsCommand {
    /// Show the source account's calendars
    List,
    /// Start syncing a calendar by name
    Add { name: String },
    /// Stop syncing a calendar by name
    Remove {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Auth { account, check } => cmd_auth(&account, check),
        _ => Err(anyhow::anyhow!("not implemented yet")),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn cmd_auth(account: &str, check: bool) -> anyhow::Result<()> {
    use api::AccessTokens;
    let cfg = config::load(&config::config_path())?;
    let client = oauth::load_client(&cfg.oauth_client)?;
    if check {
        let mut store = oauth::TokenStore::new(client);
        store.access_token(account)?;
        println!("OK {account}");
        return Ok(());
    }
    let scopes = config::required_scopes(&cfg, account);
    if scopes.is_empty() {
        anyhow::bail!("account {account:?} isn't used by any sync rule in the config");
    }
    let hint = cfg.accounts.get(account).and_then(|a| a.email.as_deref());
    let grant = oauth::run_loopback_flow(&client, &scopes, hint)?;
    let missing = oauth::missing_scopes(&grant.scope, &scopes);
    if !missing.is_empty() {
        anyhow::bail!(
            "Google didn't grant: {}. Run auth again and leave every box checked.",
            missing.join(", ")
        );
    }
    keychain::save_refresh_token(account, &grant.refresh_token)?;
    println!("Signed in {account}");
    Ok(())
}
