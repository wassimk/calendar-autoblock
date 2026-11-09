Output "demo/calendar-autoblock.gif"

Require cargo
Require python3

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 900
Set Height 550
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 50ms

Hide
Type "source demo/setup.sh && clear"
Enter
Sleep 45s
Show

Type "calendar-autoblock calendars list"
Sleep 500ms
Enter
Wait+Screen /Book club/
Sleep 3s

Type "calendar-autoblock sync --dry-run"
Sleep 500ms
Enter
Wait+Screen /nothing written/
Sleep 4s

Ctrl+L
Type "calendar-autoblock install"
Sleep 500ms
Enter
Wait+Screen /installed and loaded/
Sleep 3s

Type "calendar-autoblock status"
Sleep 500ms
Enter
Wait+Screen /Logs:/
Sleep 5s

Hide
Type "source demo/cleanup.sh"
Enter
Sleep 1s
