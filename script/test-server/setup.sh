#!/usr/bin/env bash
# Builds and runs a Mattermost server from source on an Ubuntu VM, as the test target for
# MatterLite. Idempotent: finished steps are skipped (REBUILD=1 forces the builds).
#
#   ssh andrew@ubuntu 'bash -s' < script/test-server/setup.sh
#
# Then seed test data with seed.sh. See README.md.
set -euo pipefail

MM_COMMIT="${MM_COMMIT:-5b9537164d5fe2129eea0d34b7280bc24f066c7c}"
MM_DIR="${MM_DIR:-$HOME/Projects/mattermost}"
# The address clients use. "ubuntu" is the VM's name in the Windows hosts file (the VM's own
# hostname differs). WebSocket origin checks and links depend on it.
SITE_URL="${SITE_URL:-http://ubuntu:8065}"
GO_VERSION=1.26.10      # server/.go-version
NODE_VERSION=24.11.1    # webapp/package.json "engines" (exact match, npm engine-strict)
REBUILD="${REBUILD:-0}"

NODE_DIR="$HOME/.local/opt/node-v$NODE_VERSION-linux-x64"
SERVICE=mattermost-dev
DB_NAME=mattermost_test
DB_USER=mmuser
DB_PASS=mostest_password  # the dev default in server/public/model/config.go

step() { printf '\n==> %s\n' "$*"; }

step "System packages"
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q postgresql jq oathtool curl git make gcc >/dev/null

step "Go $GO_VERSION"
if [[ "$(/usr/local/go/bin/go version 2>/dev/null)" != *"go$GO_VERSION "* ]]; then
  curl -fsSL "https://go.dev/dl/go$GO_VERSION.linux-amd64.tar.gz" -o /tmp/go.tgz
  sudo rm -rf /usr/local/go && sudo tar -C /usr/local -xzf /tmp/go.tgz && rm /tmp/go.tgz
fi
grep -q /usr/local/go/bin ~/.profile || echo 'export PATH=$PATH:/usr/local/go/bin:$HOME/go/bin' >> ~/.profile
export PATH="$PATH:/usr/local/go/bin:$HOME/go/bin"
go version

step "Node $NODE_VERSION (private copy, only for the webapp build)"
if [[ ! -x "$NODE_DIR/bin/node" ]]; then
  mkdir -p "$(dirname "$NODE_DIR")"
  curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-x64.tar.xz" | tar -xJ -C "$(dirname "$NODE_DIR")"
fi

step "Source at $MM_COMMIT"
if [[ ! -d "$MM_DIR/.git" ]]; then
  mkdir -p "$MM_DIR"
  git -C "$MM_DIR" init -q
  git -C "$MM_DIR" remote add origin https://github.com/mattermost/mattermost.git
fi
if [[ "$(git -C "$MM_DIR" rev-parse HEAD 2>/dev/null)" != "$MM_COMMIT" ]]; then
  git -C "$MM_DIR" fetch -q --depth 1 origin "$MM_COMMIT"
  git -C "$MM_DIR" checkout -q FETCH_HEAD
  REBUILD=1
fi
git -C "$MM_DIR" log --oneline -1

step "Web UI"
if [[ "$REBUILD" == 1 || ! -f "$MM_DIR/webapp/channels/dist/root.html" ]]; then
  (cd "$MM_DIR/webapp" && PATH="$NODE_DIR/bin:$PATH" npm ci --no-audit --no-fund && PATH="$NODE_DIR/bin:$PATH" npm run build) \
    > ~/mm-webapp-build.log 2>&1 || { tail -30 ~/mm-webapp-build.log; exit 1; }
fi
mkdir -p "$MM_DIR/webapp/channels/dist/files"
ln -nfs ../webapp/channels/dist "$MM_DIR/server/client"   # what `make client` does

step "Server and mmctl"
if [[ "$REBUILD" == 1 || ! -x "$MM_DIR/server/bin/mattermost" ]]; then
  (cd "$MM_DIR/server" && make setup-go-work && go build -o bin/mattermost ./cmd/mattermost && go build -o bin/mmctl ./cmd/mmctl) \
    > ~/mm-server-build.log 2>&1 || { tail -30 ~/mm-server-build.log; exit 1; }
fi
"$MM_DIR/server/bin/mattermost" version 2>/dev/null | head -1 || true
sudo ln -sf "$MM_DIR/server/bin/mmctl" /usr/local/bin/mmctl

step "PostgreSQL database $DB_NAME"
if ! sudo -u postgres psql -tAc "select 1 from pg_roles where rolname='$DB_USER'" | grep -q 1; then
  sudo -u postgres psql -q -c "CREATE ROLE $DB_USER LOGIN PASSWORD '$DB_PASS';"
fi
if ! sudo -u postgres psql -tAc "select 1 from pg_database where datname='$DB_NAME'" | grep -q 1; then
  sudo -u postgres psql -q -c "CREATE DATABASE $DB_NAME OWNER $DB_USER;"
fi

step "Host name 'ubuntu' on the VM"
# Clients reach the VM as "ubuntu" (Windows hosts file); the VM must resolve its own SiteURL
# too, e.g. for plugin tooling or server-side fetches of its own links.
getent hosts ubuntu >/dev/null || echo "127.0.1.1 ubuntu" | sudo tee -a /etc/hosts >/dev/null

step "systemd service $SERVICE"
# Only local mode comes from the environment (settings set by env are read-only in the System
# Console); everything else is patched into config/config.json below and stays editable.
sudo tee /etc/systemd/system/$SERVICE.service >/dev/null <<EOF
[Unit]
Description=Mattermost dev server (MatterLite test target)
After=network.target postgresql.service
Requires=postgresql.service

[Service]
Type=notify
User=$USER
WorkingDirectory=$MM_DIR/server
ExecStart=$MM_DIR/server/bin/mattermost
Environment=MM_SERVICESETTINGS_ENABLELOCALMODE=true
Restart=always
RestartSec=5
TimeoutStartSec=900
LimitNOFILE=49152

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable -q $SERVICE
sudo systemctl restart $SERVICE

for _ in $(seq 120); do
  curl -fsS http://localhost:8065/api/v4/system/ping >/dev/null 2>&1 && break
  sleep 2
done
curl -fsS http://localhost:8065/api/v4/system/ping; echo

step "Server configuration"
cat > /tmp/mm-config-patch.json <<EOF
{
  "ServiceSettings": {
    "SiteURL": "$SITE_URL",
    "ListenAddress": ":8065",
    "EnableUserAccessTokens": true,
    "EnableBotAccountCreation": true,
    "EnableMultifactorAuthentication": true,
    "EnableCustomEmoji": true,
    "EnableIncomingWebhooks": true,
    "EnableOutgoingWebhooks": true,
    "EnableCommands": true,
    "EnablePostUsernameOverride": true,
    "EnablePostIconOverride": true,
    "EnableLinkPreviews": true,
    "EnableLatex": true,
    "EnableInlineLatex": true,
    "TrustedProxyIPHeader": ["X-Forwarded-For", "X-Real-IP"]
  },
  "TeamSettings": {
    "EnableOpenServer": true,
    "MaxUsersPerTeam": 1000
  },
  "EmailSettings": {
    "SendEmailNotifications": false,
    "RequireEmailVerification": false,
    "EnablePreviewModeBanner": false
  },
  "PluginSettings": {
    "Enable": true,
    "EnableUploads": true,
    "RequirePluginSignature": false,
    "EnableMarketplace": false
  },
  "RateLimitSettings": { "Enable": false },
  "LogSettings": { "ConsoleLevel": "INFO", "FileLevel": "INFO" }
}
EOF
mmctl --local config patch /tmp/mm-config-patch.json >/dev/null
rm /tmp/mm-config-patch.json
mmctl --local config get ServiceSettings.SiteURL

step "HTTPS/WSS: nginx on :443 with a test CA"
# The CA lives outside ~/mm-test-server (reset.sh deletes that) because clients trust it once.
# Its name constraints limit it to this VM's names and private addresses, so trusting it on a
# client can't vouch for any other site.
CA_DIR="$HOME/mm-test-ca"
TLS_DIR=/etc/ssl/matterlite
VM_IP=$(hostname -I | awk '{print $1}')
mkdir -p "$CA_DIR" && chmod 700 "$CA_DIR"
if [[ ! -f "$CA_DIR/ca.crt" ]]; then
  openssl req -x509 -new -noenc -newkey ec -pkeyopt ec_paramgen_curve:P-256 -sha256 -days 3650 \
    -keyout "$CA_DIR/ca.key" -out "$CA_DIR/ca.crt" -subj "/O=MatterLite/CN=MatterLite Test CA" \
    -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -addext "nameConstraints=critical,permitted;DNS:ubuntu,permitted;DNS:andrew-ubuntu,permitted;DNS:localhost,permitted;IP:192.168.0.0/255.255.0.0,permitted;IP:10.0.0.0/255.0.0.0,permitted;IP:127.0.0.0/255.0.0.0" \
    2>/dev/null
fi
sudo mkdir -p "$TLS_DIR"
# (Re)issue the server certificate when missing, expiring within 30 days, or not covering
# the VM's current address.
san=$(sudo openssl x509 -in "$TLS_DIR/server.crt" -noout -ext subjectAltName 2>/dev/null || true)
if ! sudo openssl x509 -in "$TLS_DIR/server.crt" -noout -checkend $((30 * 86400)) >/dev/null 2>&1 \
   || [[ "$san," != *"IP Address:$VM_IP,"* ]]; then
  tmp=$(mktemp -d)
  openssl req -new -noenc -newkey ec -pkeyopt ec_paramgen_curve:P-256 -sha256 \
    -keyout "$tmp/server.key" -out "$tmp/server.csr" -subj "/CN=ubuntu" 2>/dev/null
  printf '%s\n' "subjectAltName=DNS:ubuntu,DNS:andrew-ubuntu,DNS:localhost,IP:$VM_IP,IP:127.0.0.1" \
    "basicConstraints=critical,CA:FALSE" "keyUsage=critical,digitalSignature" \
    "extendedKeyUsage=serverAuth" "subjectKeyIdentifier=hash" "authorityKeyIdentifier=keyid" > "$tmp/ext.cnf"
  openssl x509 -req -sha256 -days 397 -in "$tmp/server.csr" -CA "$CA_DIR/ca.crt" -CAkey "$CA_DIR/ca.key" \
    -CAcreateserial -CAserial "$CA_DIR/ca.srl" -extfile "$tmp/ext.cnf" -out "$tmp/server.crt" 2>/dev/null
  sudo install -m 644 "$tmp/server.crt" "$TLS_DIR/server.crt"
  sudo install -m 600 "$tmp/server.key" "$TLS_DIR/server.key"
  rm -rf "$tmp"
fi
sudo install -m 644 "$CA_DIR/ca.crt" "$TLS_DIR/ca.crt"
# The VM itself trusts the CA too (curl https://localhost, plugin tooling).
sudo install -m 644 "$CA_DIR/ca.crt" /usr/local/share/ca-certificates/matterlite-test-ca.crt
sudo update-ca-certificates >/dev/null 2>&1

sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q nginx >/dev/null
# Mattermost's recommended proxy setup (docs: "Configure NGINX as a proxy for Mattermost").
sudo tee /etc/nginx/sites-available/mattermost >/dev/null <<'EOF'
upstream mattermost {
    server 127.0.0.1:8065;
    keepalive 32;
}

server {
    listen 80 default_server;
    listen [::]:80 default_server;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl default_server;
    listen [::]:443 ssl default_server;
    http2 on;
    server_name ubuntu andrew-ubuntu localhost;

    ssl_certificate     /etc/ssl/matterlite/server.crt;
    ssl_certificate_key /etc/ssl/matterlite/server.key;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_session_cache shared:SSL:10m;

    # FileSettings.MaxFileSize is 100 MB; leave room for multipart overhead.
    client_max_body_size 110M;

    location ~ /api/v[0-9]+/(users/)?websocket$ {
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Frame-Options SAMEORIGIN;
        proxy_buffers 256 16k;
        proxy_buffer_size 16k;
        client_body_timeout 60s;
        send_timeout 300s;
        lingering_timeout 5s;
        proxy_connect_timeout 90s;
        proxy_send_timeout 300s;
        # The server pings every 60 s (pingInterval in server/channels/app/platform/web_conn.go).
        proxy_read_timeout 90s;
        proxy_http_version 1.1;
        proxy_pass http://mattermost;
    }

    location / {
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Frame-Options SAMEORIGIN;
        proxy_buffers 256 16k;
        proxy_buffer_size 16k;
        proxy_read_timeout 600s;
        proxy_http_version 1.1;
        proxy_pass http://mattermost;
    }
}
EOF
sudo ln -sf /etc/nginx/sites-available/mattermost /etc/nginx/sites-enabled/mattermost
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t -q
sudo systemctl enable -q nginx
sudo systemctl reload-or-restart nginx
curl -fsS https://localhost/api/v4/system/ping >/dev/null && echo "https://localhost OK"
echo "Trust $CA_DIR/ca.crt on clients (see docs/MatterLite-TestServer.md §12)"

step "Done: $SITE_URL and https://ubuntu (logs: journalctl -u $SERVICE -f, $MM_DIR/server/mattermost.log)"
