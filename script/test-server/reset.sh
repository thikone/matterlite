#!/usr/bin/env bash
# Wipes the test server (database, uploaded files, installed plugins) and seeds it again.
#
#   ssh andrew@ubuntu 'bash -s' < script/test-server/reset.sh
#   ssh andrew@ubuntu 'bash -s' < script/test-server/seed.sh
#
# Server configuration (config/config.json) is kept.
set -euo pipefail

MM_DIR="${MM_DIR:-$HOME/Projects/mattermost}"
SERVICE=mattermost-dev

sudo systemctl stop $SERVICE
sudo -u postgres psql -q -c "DROP DATABASE IF EXISTS mattermost_test WITH (FORCE);" \
  -c "CREATE DATABASE mattermost_test OWNER mmuser;"
rm -rf "$MM_DIR/server/data" "$MM_DIR/server/plugins" "$MM_DIR/server/client/plugins" "$HOME/mm-test-server"
sudo systemctl start $SERVICE
for _ in $(seq 120); do
  curl -fsS http://localhost:8065/api/v4/system/ping >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS http://localhost:8065/api/v4/system/ping; echo
echo "Reset done. Seed with seed.sh."
