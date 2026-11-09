# Sourced (hidden) by the tape: builds the binary and stages a sandbox with fake Google and fake launchd.
cargo build --release -q || return
export DEMO_HOME="$(mktemp -d)"
mkdir -p "$DEMO_HOME/.config/calendar-autoblock"
cp demo/config.toml demo/client_secret.json "$DEMO_HOME/.config/calendar-autoblock/"
chmod 600 "$DEMO_HOME/.config/calendar-autoblock/client_secret.json"

python3 demo/fake_google.py 8765 >/dev/null 2>&1 &
export DEMO_SERVER=$!

# Throwaway tokens under the demo's own account names, readable without a prompt
for account in home office; do
  security add-generic-password -U -A -s calendar-autoblock -a "$account" -w demo-token
done

export CALENDAR_AUTOBLOCK_HOME="$DEMO_HOME"
export CALENDAR_AUTOBLOCK_API_BASE="http://127.0.0.1:8765"
export PATH="$PWD/demo/bin:$PWD/target/release:$PATH"
export PS1='$ '

# The first read of a Keychain item that `security` just wrote takes several seconds; pay it off camera
for account in home office; do
  calendar-autoblock auth "$account" --check >/dev/null 2>&1
done

# Show sandbox paths as ~
calendar-autoblock() { command calendar-autoblock "$@" | sed "s|$DEMO_HOME|~|g"; }
