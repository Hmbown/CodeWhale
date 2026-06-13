#!/usr/bin/env bash
# EXPERIMENTAL — DigitalOcean smoke-lab provisioning for the CodeWhale
# remote workbench (issue #1990 "clearly documented better alternative"
# clause). Creates ONE Ubuntu 24.04 droplet plus a cloud firewall that
# allows inbound SSH only. Prints the monthly price from the DO API and
# requires a typed "yes" before creating anything billable.
#
# Auth: doctl must be authenticated. Either
#   doctl auth init                      # paste token interactively, or
#   export DIGITALOCEAN_ACCESS_TOKEN=... # doctl reads this env var
#
# Usage:
#   bash scripts/remote-smoke/digitalocean/provision.sh
#
# Tunables (env):
#   DROPLET_NAME    default codewhale-smoke
#   DO_REGION       default sfo3 (San Francisco)
#   DROPLET_SIZE    default s-1vcpu-2gb (~$12/mo; prebuilt binaries mean no
#                   Rust build, so 1 vCPU / 2 GB is enough for the smoke.
#                   Use s-2vcpu-2gb/s-2vcpu-4gb for a longer-lived host.)
#   DROPLET_IMAGE   default ubuntu-24-04-x64
#   SSH_PUBKEY      default ~/.ssh/id_ed25519.pub (imported if not present)
#   RESTRICT_SSH_TO_MY_IP  default true (firewall source = caller IP /32)
set -euo pipefail

DROPLET_NAME="${DROPLET_NAME:-codewhale-smoke}"
DO_REGION="${DO_REGION:-sfo3}"
DROPLET_SIZE="${DROPLET_SIZE:-s-1vcpu-2gb}"
DROPLET_IMAGE="${DROPLET_IMAGE:-ubuntu-24-04-x64}"
SSH_PUBKEY="${SSH_PUBKEY:-$HOME/.ssh/id_ed25519.pub}"
SSH_KEY_NAME="${SSH_KEY_NAME:-${DROPLET_NAME}-key}"
FIREWALL_NAME="${FIREWALL_NAME:-${DROPLET_NAME}-ssh-only}"
RESTRICT_SSH_TO_MY_IP="${RESTRICT_SSH_TO_MY_IP:-true}"

command -v doctl >/dev/null || { echo "doctl is required (brew install doctl)" >&2; exit 1; }
doctl account get >/dev/null || { echo "doctl is not authenticated; run 'doctl auth init' or set DIGITALOCEAN_ACCESS_TOKEN" >&2; exit 1; }
[[ -f "$SSH_PUBKEY" ]] || { echo "SSH public key not found: $SSH_PUBKEY" >&2; exit 1; }

echo "== Preflight =="
[[ "$(doctl compute region list --format Slug,Available --no-header | awk -v r="$DO_REGION" '$1 == r {print $2}')" == "true" ]] \
  || { echo "Region ${DO_REGION} not available" >&2; exit 1; }

read -r PRICE VCPUS MEM DISK < <(doctl compute size list \
  --format Slug,PriceMonthly,VCPUs,Memory,Disk --no-header \
  | awk -v s="$DROPLET_SIZE" '$1 == s {print $2, $3, $4, $5}')
[[ -n "${PRICE:-}" ]] || { echo "Size ${DROPLET_SIZE} not found" >&2; exit 1; }

echo "Region:        $DO_REGION"
echo "Droplet:       $DROPLET_NAME"
echo "Image:         $DROPLET_IMAGE"
echo "Size:          $DROPLET_SIZE (${VCPUS} vCPU / ${MEM} MB RAM / ${DISK} GB disk)"
echo "Monthly price: \$$PRICE USD (billed hourly until destroyed)"
echo "SSH key:       $SSH_PUBKEY -> '$SSH_KEY_NAME'"
echo "Firewall:      $FIREWALL_NAME (inbound 22/tcp only)"
echo
read -r -p "Create this droplet and start billing? Type 'yes' to proceed: " CONFIRM
[[ "$CONFIRM" == "yes" ]] || { echo "Aborted; nothing created."; exit 1; }

echo "== Import SSH key =="
KEY_ID=$(doctl compute ssh-key list --format ID,Name --no-header | awk -v n="$SSH_KEY_NAME" '$2 == n {print $1; exit}')
if [[ -z "$KEY_ID" ]]; then
  KEY_ID=$(doctl compute ssh-key import "$SSH_KEY_NAME" --public-key-file "$SSH_PUBKEY" --format ID --no-header)
  echo "imported $SSH_KEY_NAME (id $KEY_ID)"
else
  echo "key $SSH_KEY_NAME already exists (id $KEY_ID); reusing"
fi

echo "== Create droplet =="
doctl compute droplet create "$DROPLET_NAME" \
  --region "$DO_REGION" \
  --image "$DROPLET_IMAGE" \
  --size "$DROPLET_SIZE" \
  --ssh-keys "$KEY_ID" \
  --tag-name codewhale-smoke \
  --wait >/dev/null
DROPLET_ID=$(doctl compute droplet list --format ID,Name --no-header | awk -v n="$DROPLET_NAME" '$2 == n {print $1; exit}')
IP=$(doctl compute droplet get "$DROPLET_ID" --format PublicIPv4 --no-header)
echo "created $DROPLET_NAME (id $DROPLET_ID, $IP)"

echo "== Cloud firewall: SSH only =="
SRC="0.0.0.0/0,address:::/0"
if [[ "$RESTRICT_SSH_TO_MY_IP" == "true" ]]; then
  MYIP=$(curl -fsS https://api.ipify.org)
  SRC="${MYIP}/32"
fi
if ! doctl compute firewall list --format Name --no-header | grep -qx "$FIREWALL_NAME"; then
  doctl compute firewall create \
    --name "$FIREWALL_NAME" \
    --inbound-rules "protocol:tcp,ports:22,address:${SRC}" \
    --outbound-rules "protocol:tcp,ports:all,address:0.0.0.0/0,address:::/0 protocol:udp,ports:all,address:0.0.0.0/0,address:::/0 protocol:icmp,address:0.0.0.0/0,address:::/0" \
    --droplet-ids "$DROPLET_ID" >/dev/null
  echo "firewall $FIREWALL_NAME created: inbound 22/tcp from ${SRC}, all else blocked"
else
  FW_ID=$(doctl compute firewall list --format ID,Name --no-header | awk -v n="$FIREWALL_NAME" '$2 == n {print $1; exit}')
  doctl compute firewall add-droplets "$FW_ID" --droplet-ids "$DROPLET_ID"
  echo "existing firewall $FIREWALL_NAME attached"
fi

echo
echo "== Done =="
echo "Droplet:   $DROPLET_NAME ($DO_REGION)"
echo "Public IP: $IP"
echo "SSH:       ssh -i ${SSH_PUBKEY%.pub} root@${IP}"
echo "           (DO Ubuntu images log in as root, not ubuntu)"
echo
echo "Teardown when finished (stops billing):"
echo "  bash scripts/remote-smoke/digitalocean/teardown.sh"
