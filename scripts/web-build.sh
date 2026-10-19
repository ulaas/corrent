#!/usr/bin/env bash
# Builds the web client (web/) into web/dist, which correntd embeds. Needs Node 20+ and npm.
#   scripts/web-build.sh            install exact dependencies, check, test and build
#   scripts/web-build.sh --no-ci    reuse web/node_modules (faster while developing)
set -euo pipefail
cd "$(dirname "$0")/../web"
if [[ "${1:-}" != "--no-ci" ]]; then
  npm ci --no-audit --no-fund
fi
npm run check
npm run build
echo "web/dist is up to date ($(du -sh dist | cut -f1))"
