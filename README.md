# calendar-autoblock

Block time on your work Google Calendar when personal appointments fall during your work day. It runs in the background on your Mac and copies each qualifying personal event onto your work calendar as a busy block titled **Personal Commitment**, with no other details.

![calendar-autoblock demo](demo/calendar-autoblock.gif)

## 💡 How It Works

Every 5 minutes, calendar-autoblock reads the next 90 days of the personal calendars you pick, decides which events overlap your work hours (plus a margin), and makes your work calendar match:

- A new or moved appointment gets a matching block within about 5 minutes.
- A deleted or declined appointment loses its block.
- A block you delete by hand stays deleted.
- A block you move or edit by hand snaps back to match its appointment. Move the appointment instead.

Each block is a public, busy event titled `Personal Commitment`. It has no description, location, attendees, video link, or reminders, and it never sends invitations or updates.

Which personal events count:

| Event | Copied? |
|---|---|
| Timed event that overlaps your work hours plus the margin | Yes |
| Accepted or "maybe" invitation | Yes |
| Out of office | Yes |
| All-day event marked busy | Yes |
| Event marked free, including most all-day events | No |
| Declined or unanswered invitation | No |
| Working location, focus time, birthdays, events from Gmail | No |
| Anything that has already ended | Never touched |

## 🔒 Privacy

calendar-autoblock runs only on your Mac and talks only to Google Calendar. Nothing is sent anywhere else.

- **Personal account:** read-only access to events on calendars you own (`calendar.events.owned.readonly`) and to the names of your calendars (`calendar.calendarlist.readonly`).
- **Work account:** access to events on calendars you own (`calendar.events.owned`), used only to create, update, and delete its own blocks. It finds them by a hidden tag and never changes any other event.
- Blocks copy only the start and end time. Titles, descriptions, locations, and attendees stay on your personal calendar.
- Sign-in tokens are stored in the macOS Keychain. Your OAuth client file stays in `~/.config/calendar-autoblock/`.

You bring your own Google OAuth client, so no one else's app is involved.

## 🛠️ Install

```shell
brew install wassimk/tap/calendar-autoblock
```

Requires macOS. Release builds are signed and notarized.

### 1. Create a Google OAuth client

You need your own OAuth client from Google Cloud. It takes about ten minutes, once.

1. Go to [console.cloud.google.com/projectcreate](https://console.cloud.google.com/projectcreate) and create a project, for example `calendar-autoblock`.
2. Enable the [Google Calendar API](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com) for that project.
3. Open **Google Auth Platform** and click **Get started**:
   - App name: `calendar-autoblock`, plus your support email.
   - Audience: **External**. Internal only works when both accounts are in the same Google Workspace.
   - Contact email, then agree to the policy and create.
4. **Branding:** add a home page and privacy policy link (this repo's URL and `#privacy` work), and add `github.com` under Authorized domains. Google won't let you publish without them.
5. **Data Access:** add these three scopes:
   - `https://www.googleapis.com/auth/calendar.events.owned.readonly`
   - `https://www.googleapis.com/auth/calendar.calendarlist.readonly`
   - `https://www.googleapis.com/auth/calendar.events.owned`
6. **Audience:** click **Publish app**. In Testing mode Google expires sign-ins after 7 days, which would quietly stop the sync every week. You don't need verification, but you'll see an "unverified app" warning when you sign in.
7. **Clients:** create a client of type **Desktop app**, then **Download JSON** before closing the dialog. Google won't show the secret again.
8. Move it into place:

   ```shell
   mkdir -p ~/.config/calendar-autoblock
   mv ~/Downloads/client_secret_*.json ~/.config/calendar-autoblock/client_secret.json
   chmod 600 ~/.config/calendar-autoblock/client_secret.json
   ```

### 2. Run setup

```shell
calendar-autoblock setup
```

Setup signs in to your personal and work accounts (a browser opens for each), lists your personal calendars so you can pick which ones block your work time, and asks for your work hours. On Google's "unverified app" screen, choose **Advanced**, then **Go to calendar-autoblock**, and leave every box checked.

### 3. Preview, then install

```shell
calendar-autoblock sync --dry-run
calendar-autoblock install
```

`install` adds a LaunchAgent so the sync runs at login and every 5 minutes after. The first time it reads your sign-in tokens, macOS asks to allow Keychain access. Choose **Always Allow**.

## 💻 Usage

```
$ calendar-autoblock --help

Block time on your work Google Calendar for personal appointments

Usage: calendar-autoblock <COMMAND>

Commands:
  setup      Guided first run: sign in, pick calendars, set work hours
  calendars  List, add, or remove source calendars
  auth       Sign in to an account, or check an existing sign-in
  sync       Run one sync pass
  cleanup    Delete every future copy this tool created
  start      Run the sync loop in the foreground
  install    Install the LaunchAgent for automatic startup
  uninstall  Uninstall the LaunchAgent
  stop       Stop the background service
  restart    Restart the background service
  status     Show running state, last sync, auth health, and log paths
```

Add or remove a personal calendar later:

```shell
calendar-autoblock calendars list
calendar-autoblock calendars add Family
calendar-autoblock calendars remove Family   # offers to delete its future blocks
```

Remove every future block, for example before uninstalling:

```shell
calendar-autoblock cleanup --dry-run
calendar-autoblock cleanup
```

## ⚙️ Config

`setup` writes `~/.config/calendar-autoblock/config.toml`. You can edit it by hand:

```toml
oauth_client = "~/.config/calendar-autoblock/client_secret.json"
label = "Personal Commitment"   # title of every block
horizon_days = 90               # how far ahead to sync
poll_seconds = 300

[work_hours]
timezone = "America/Chicago"
days = ["mon", "tue", "wed", "thu"]
start = "08:00"
end = "17:00"
margin_before = "1h"            # appointments this close to your day still block it
margin_after = "1h"
```

The running service rereads the config on every pass, so changes apply within 5 minutes.

## 📋 Logs

When running as a LaunchAgent:

- `~/Library/Logs/calendar-autoblock/stdout.log`
- `~/Library/Logs/calendar-autoblock/stderr.log`

The log notes when the sync goes offline and comes back, auth problems, and each pass that changed something. Run `calendar-autoblock start --verbose` to log every pass.

## 🔧 Troubleshooting

Start with:

```shell
calendar-autoblock status
```

- **`Auth error (work): ... run: calendar-autoblock auth work`.** The sign-in was revoked or expired. Run the command it shows. `calendar-autoblock auth work --check` confirms a sign-in without opening the browser.
- **`Google didn't grant: ...`.** A box was unchecked on Google's consent screen. Run `auth` again and leave every box checked.
- **`not owned by personal`.** That calendar is shared with you, and the read-only scope only covers calendars you own.
- **`Offline since ...`.** Your Mac couldn't reach Google. It catches up on the next pass after reconnecting, and it never deletes a block because a read failed.

## 📝 Good to Know

- It runs only while your Mac is awake. New appointments made while it's off appear on the next pass after it wakes.
- Recurring appointments are copied one occurrence at a time, so declining or deleting one occurrence removes just that block.
- Deleting a whole series removes its blocks across the whole sync window (the next 90 days by default) on the next pass.
- If the same event is on two of your personal calendars, you get one block.
