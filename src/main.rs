mod api;
mod config;
mod daemon;
mod eligibility;
mod event;
mod google;
mod keychain;
mod launchagent;
mod oauth;
mod reconcile;
mod setup;
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
    if let Err(e) = keychain::init() {
        eprintln!("error: couldn't open the Keychain: {e:#}");
        std::process::exit(1);
    }
    let result = match cli.command {
        Command::Auth { account, check } => cmd_auth(&account, check),
        Command::Sync { dry_run } => cmd_sync(dry_run),
        Command::Cleanup { dry_run, yes } => cmd_cleanup(dry_run, yes),
        Command::Start { verbose } => daemon::run_daemon(&config::config_path(), verbose),
        Command::Install { force } => launchagent::install_launchagent(force),
        Command::Uninstall => launchagent::uninstall_launchagent(),
        Command::Stop => launchagent::stop_launchagent(),
        Command::Restart => launchagent::restart_launchagent(),
        Command::Status => launchagent::show_status(),
        Command::Setup => setup::run_setup(),
        Command::Calendars { command } => match command {
            CalendarsCommand::List => setup::calendars_list(),
            CalendarsCommand::Add { name } => setup::calendars_add(&name),
            CalendarsCommand::Remove { name, yes } => setup::calendars_remove(&name, yes),
        },
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
    setup::sign_in(&client, account, &scopes, hint)?;
    Ok(())
}

fn cmd_sync(dry_run: bool) -> anyhow::Result<()> {
    let cfg = config::load(&config::config_path())?;
    let mut api = daemon::build_api(&cfg)?;
    let (report, _) = daemon::pass_once(&cfg, &mut api, dry_run)?;
    for action in &report.actions {
        println!("{}", describe(&cfg, action));
    }
    for (target, err) in &report.failures {
        eprintln!("failed {target}: {err}");
    }
    let c = report.counts;
    if dry_run {
        println!(
            "dry run: {} change(s) planned, nothing written",
            report.actions.len()
        );
    } else {
        println!(
            "{} created, {} patched, {} deleted",
            c.created, c.patched, c.deleted
        );
    }
    Ok(())
}

fn describe(cfg: &config::Config, action: &reconcile::Action) -> String {
    use reconcile::{Action, DeleteReason};
    let when = |d: &reconcile::Desired| {
        let tz = cfg.work_hours.timezone;
        match d.source.bounds(tz) {
            Some((s, e)) => format!(
                "{} {}",
                s.with_timezone(&tz).format("%Y-%m-%d %H:%M"),
                e.with_timezone(&tz).format("%H:%M")
            ),
            None => "?".into(),
        }
    };
    let from = |d: &reconcile::Desired| {
        cfg.sync
            .iter()
            .find(|r| r.from.calendar == d.source_calendar)
            .map_or(d.source_calendar.as_str(), |r| r.from.display_name())
            .to_string()
    };
    match action {
        Action::Create { desired } => format!("create {} ({})", when(desired), from(desired)),
        Action::Patch { desired, .. } => format!("patch  {} ({})", when(desired), from(desired)),
        Action::Delete {
            dest_event_id,
            reason,
            ..
        } => {
            let why = match reason {
                DeleteReason::SourceGone => "source gone or outside work hours",
                DeleteReason::Duplicate => "duplicate",
            };
            format!("delete copy {dest_event_id} ({why})")
        }
    }
}

fn cmd_cleanup(dry_run: bool, yes: bool) -> anyhow::Result<()> {
    let cfg = config::load(&config::config_path())?;
    let mut api = daemon::build_api(&cfg)?;
    let items = sync::cleanup_actions(&cfg, &mut api, chrono::Utc::now(), None)?;
    let tz = cfg.work_hours.timezone;
    for (account, calendar, copy) in &items {
        let start = copy
            .bounds(tz)
            .map(|(s, _)| s.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        println!("{start}  {account}/{calendar}  {}", copy.id);
    }
    if items.is_empty() {
        println!("no future copies to delete");
        return Ok(());
    }
    if dry_run {
        println!("dry run: {} copies would be deleted", items.len());
        return Ok(());
    }
    if launchagent::is_running() {
        anyhow::bail!(
            "the background service is running and would recreate these blocks within 5 minutes; \
             run `calendar-autoblock stop` (or `uninstall`) first"
        );
    }
    if !yes && !setup::confirm(&format!("Delete {} copies?", items.len()))? {
        println!("nothing deleted");
        return Ok(());
    }
    daemon::delete_and_report(&mut api, &items, "run cleanup again")
}
