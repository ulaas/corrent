#!/bin/sh
# Install and start the Corrent Server launchd agent for the user running this script.
set -e
label=io.github.ulaas.correntd
mkdir -p "$HOME/Library/LaunchAgents"
cp "/usr/local/share/corrent-server/$label.plist" "$HOME/Library/LaunchAgents/$label.plist"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/$label.plist"
echo "started $label; data in ~/Library/Application Support/Corrent Server"
