# Demo

The README GIF is rendered with [VHS](https://github.com/charmbracelet/vhs) from `calendar-autoblock.tape`.

## Requirements

- `vhs`
- A Rust toolchain (the tape builds `target/release/calendar-autoblock`)
- Python 3.9 or later (for the fake Google server)
- JetBrainsMono Nerd Font

## Rendering

From the repo root:

```shell
vhs demo/calendar-autoblock.tape
```

This overwrites `demo/calendar-autoblock.gif`. To compress it afterward:

```shell
gifsicle --lossy=80 -k 128 -O2 -Okeep-empty demo/calendar-autoblock.gif -o demo/calendar-autoblock.gif
```

## How it works

No Google account, real calendar, or real LaunchAgent is involved.

- `setup.sh` (sourced while the recording is hidden) builds the binary, copies `config.toml` and a fake `client_secret.json` into a temp directory, and points `CALENDAR_AUTOBLOCK_HOME` at it so the config, state, logs, and plist all live there. A shell function rewrites that temp path to `~` in the output.
- `fake_google.py` stands in for the Calendar API and the token endpoint on `127.0.0.1:8765`. `CALENDAR_AUTOBLOCK_API_BASE` points the binary at it. It serves a made-up Appointments calendar whose events are placed on the coming Monday through Thursday, plus events that should be skipped: a dinner outside work hours, a free lunch, an unanswered invite, and a weekend brunch.
- `bin/launchctl` is first on `PATH`. It runs `calendar-autoblock start` in the background for `bootstrap` and answers `list` and `bootout`, so `install` and `status` work without touching launchd.
- Sign-in tokens come from two throwaway Keychain items (accounts `home` and `office`) that `setup.sh` adds and `cleanup.sh` deletes. The first read of a freshly written item takes several seconds, so setup reads each one once while hidden.
- `cleanup.sh` stops the fake service and server, deletes the Keychain items, and removes the temp directory.
