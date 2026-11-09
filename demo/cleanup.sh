# Sourced (hidden) at the end of the tape: stops the fakes and removes everything setup.sh made.
launchctl bootout demo 2>/dev/null
kill "$DEMO_SERVER" 2>/dev/null
for account in home office; do
  security delete-generic-password -s calendar-autoblock -a "$account" >/dev/null 2>&1
done
rm -rf "$DEMO_HOME"
