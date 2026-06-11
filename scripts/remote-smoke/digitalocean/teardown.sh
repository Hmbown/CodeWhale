#!/usr/bin/env bash
# EXPERIMENTAL — tears down the CodeWhale DigitalOcean smoke lab and lists
# anything billable that remains (droplets, volumes, snapshots, reserved
# IPs). Safe to re-run; prints what it finds before deleting.
set -euo pipefail

DROPLET_NAME="${DROPLET_NAME:-codewhale-smoke}"
SSH_KEY_NAME="${SSH_KEY_NAME:-${DROPLET_NAME}-key}"
FIREWALL_NAME="${FIREWALL_NAME:-${DROPLET_NAME}-ssh-only}"

command -v doctl >/dev/null || { echo "doctl is required" >&2; exit 1; }
doctl account get >/dev/null || { echo "doctl is not authenticated" >&2; exit 1; }

echo "== Current droplets =="
doctl compute droplet list --format ID,Name,Status,Region,SizeSlug

DROPLET_ID=$(doctl compute droplet list --format ID,Name --no-header | awk -v n="$DROPLET_NAME" '$2 == n {print $1; exit}')
if [[ -n "$DROPLET_ID" ]]; then
  read -r -p "Destroy droplet '${DROPLET_NAME}' (id ${DROPLET_ID})? Type 'yes': " CONFIRM
  [[ "$CONFIRM" == "yes" ]] || { echo "Aborted."; exit 1; }
  doctl compute droplet delete "$DROPLET_ID" --force
  echo "destroyed droplet ${DROPLET_NAME}"
else
  echo "droplet ${DROPLET_NAME} not found (already destroyed?)"
fi

FW_ID=$(doctl compute firewall list --format ID,Name --no-header | awk -v n="$FIREWALL_NAME" '$2 == n {print $1; exit}')
if [[ -n "$FW_ID" ]]; then
  doctl compute firewall delete "$FW_ID" --force
  echo "deleted firewall ${FIREWALL_NAME}"
fi

KEY_ID=$(doctl compute ssh-key list --format ID,Name --no-header | awk -v n="$SSH_KEY_NAME" '$2 == n {print $1; exit}')
if [[ -n "$KEY_ID" ]]; then
  doctl compute ssh-key delete "$KEY_ID" --force
  echo "deleted ssh key ${SSH_KEY_NAME}"
fi

echo "== Leftover billable resources check =="
echo "-- droplets:"
doctl compute droplet list --format ID,Name,Status
echo "-- volumes:"
doctl compute volume list --format ID,Name,Size
echo "-- snapshots:"
doctl compute snapshot list --format ID,Name,ResourceType
echo "-- reserved IPs (billed when unassigned):"
doctl compute reserved-ip list --format IP,DropletID
echo
echo "If all lists above are empty, DigitalOcean billing for this lab is fully stopped."
