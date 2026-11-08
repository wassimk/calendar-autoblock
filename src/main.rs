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
    let _cli = Cli::parse();
    eprintln!("not implemented yet");
    std::process::exit(1);
}
