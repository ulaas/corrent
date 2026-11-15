#!/bin/sh
# Remove Corrent Server installed by the pkg: run with sudo. Stops the agent of the logged-in user
# and removes the programs. The data (~/Library/Application Support/Corrent Server) is kept.
set -e
label=io.github.ulaas.correntd
[ "$(id -u)" = 0 ] || { echo "run with sudo"; exit 1; }
user=$(stat -f%Su /dev/console)
if [ -n "$user" ] && [ "$user" != root ] && [ "$user" != loginwindow ]; then
  uid=$(id -u "$user")
  launchctl bootout "gui/$uid/$label" 2>/dev/null || true
  home=$(dscl . -read "/Users/$user" NFSHomeDirectory | awk '{print $2}')
  rm -f "$home/Library/LaunchAgents/$label.plist"
fi
rm -f /usr/local/bin/correntd /usr/local/bin/corrent-mpvw
rm -rf /usr/local/bin/corrent-lib /usr/local/share/corrent-server
pkgutil --forget "$label" >/dev/null 2>&1 || true
echo "removed Corrent Server (data kept)"
