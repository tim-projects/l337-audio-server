#!/bin/bash
# Install the L337 Audio Server as a systemd service
set -e

if [ "$EUID" -ne 0 ]; then
  echo "Please run as root (or with sudo)"
  exit
fi

USER_NAME="l337"
INSTALL_DIR="/opt/l337-audio-server"
SERVICE_FILE="/etc/systemd/system/l337-audio.service"

echo "Creating dedicated user $USER_NAME..."
if ! id "$USER_NAME" &>/dev/null; then
  useradd -r -s /usr/sbin/nologin "$USER_NAME"
fi

echo "Installing L337 Audio Server to $INSTALL_DIR..."
mkdir -p $INSTALL_DIR
cp -r . $INSTALL_DIR
chown -R $USER_NAME:$USER_NAME $INSTALL_DIR

echo "Creating systemd service..."
cat <<EOF > $SERVICE_FILE
[Unit]
Description=L337 Audio Server
After=network.target

[Service]
User=$USER_NAME
Group=$USER_NAME
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/scripts/run-server.sh
Restart=always

[Install]
WantedBy=multi-user.target
EOF

echo "Reloading systemd, enabling and starting the service..."
systemctl daemon-reload
systemctl enable l337-audio.service
systemctl start l337-audio.service

echo "L337 Audio Server service is now running under user $USER_NAME."
