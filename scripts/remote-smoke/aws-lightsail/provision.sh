#!/usr/bin/env bash
# EXPERIMENTAL — AWS Lightsail smoke-lab provisioning for the CodeWhale
# remote workbench (issue #1990). Creates ONE Ubuntu 24.04 Lightsail
# instance with SSH-only firewall. Prints every step and the monthly price
# from the Lightsail API, then requires an explicit "yes" before creating
# anything that costs money.
#
# Usage:
#   AWS_REGION=us-east-1 bash scripts/aws-lightsail/provision.sh
#
# Tunables (env):
#   INSTANCE_NAME   default codewhale-smoke
#   BUNDLE_ID       default medium_3_0 (2 vCPU / 4 GB — docs/REMOTE_VM_US.md default)
#   BLUEPRINT_ID    default ubuntu_24_04
#   SSH_PUBKEY      default ~/.ssh/id_ed25519.pub (imported as key pair)
#   RESTRICT_SSH_TO_MY_IP  default true (firewall cidr = caller IP /32)
set -euo pipefail

INSTANCE_NAME="${INSTANCE_NAME:-codewhale-smoke}"
BUNDLE_ID="${BUNDLE_ID:-medium_3_0}"
BLUEPRINT_ID="${BLUEPRINT_ID:-ubuntu_24_04}"
SSH_PUBKEY="${SSH_PUBKEY:-$HOME/.ssh/id_ed25519.pub}"
KEY_PAIR_NAME="${KEY_PAIR_NAME:-${INSTANCE_NAME}-key}"
RESTRICT_SSH_TO_MY_IP="${RESTRICT_SSH_TO_MY_IP:-true}"

command -v aws >/dev/null || { echo "aws CLI is required" >&2; exit 1; }
aws sts get-caller-identity >/dev/null || { echo "aws is not authenticated; run 'aws configure' or 'aws sso login'" >&2; exit 1; }

REGION="${AWS_REGION:-$(aws configure get region || true)}"
[[ -n "${REGION}" ]] || { echo "Set AWS_REGION (e.g. us-east-1)" >&2; exit 1; }

echo "== Preflight =="
aws lightsail get-blueprints --region "$REGION" \
  --query "blueprints[?blueprintId=='${BLUEPRINT_ID}'].[blueprintId,name]" --output text \
  | grep -q . || { echo "Blueprint ${BLUEPRINT_ID} not found in ${REGION}" >&2; exit 1; }

PRICE=$(aws lightsail get-bundles --region "$REGION" \
  --query "bundles[?bundleId=='${BUNDLE_ID}'].price | [0]" --output text)
SPECS=$(aws lightsail get-bundles --region "$REGION" \
  --query "bundles[?bundleId=='${BUNDLE_ID}'].[cpuCount,ramSizeInGb,diskSizeInGb] | [0]" --output text)
[[ "$PRICE" != "None" && -n "$PRICE" ]] || { echo "Bundle ${BUNDLE_ID} not found in ${REGION}" >&2; exit 1; }

[[ -f "$SSH_PUBKEY" ]] || { echo "SSH public key not found: $SSH_PUBKEY" >&2; exit 1; }

echo "Region:        $REGION"
echo "Instance:      $INSTANCE_NAME"
echo "Blueprint:     $BLUEPRINT_ID"
echo "Bundle:        $BUNDLE_ID (vCPU/RAM-GB/Disk-GB: $SPECS)"
echo "Monthly price: \$$PRICE USD (billed hourly until deleted)"
echo "SSH key:       $SSH_PUBKEY -> key pair '$KEY_PAIR_NAME'"
echo
read -r -p "Create this instance and start billing? Type 'yes' to proceed: " CONFIRM
[[ "$CONFIRM" == "yes" ]] || { echo "Aborted; nothing created."; exit 1; }

echo "== Import SSH key pair =="
if ! aws lightsail get-key-pair --region "$REGION" --key-pair-name "$KEY_PAIR_NAME" >/dev/null 2>&1; then
  aws lightsail import-key-pair --region "$REGION" \
    --key-pair-name "$KEY_PAIR_NAME" \
    --public-key-base64 "$(base64 < "$SSH_PUBKEY")" >/dev/null
  echo "imported $KEY_PAIR_NAME"
else
  echo "key pair $KEY_PAIR_NAME already exists; reusing"
fi

echo "== Create instance =="
AZ=$(aws lightsail get-regions --include-availability-zones --region "$REGION" \
  --query "regions[?name=='${REGION}'].availabilityZones[0].zoneName | [0]" --output text)
aws lightsail create-instances --region "$REGION" \
  --instance-names "$INSTANCE_NAME" \
  --availability-zone "$AZ" \
  --blueprint-id "$BLUEPRINT_ID" \
  --bundle-id "$BUNDLE_ID" \
  --key-pair-name "$KEY_PAIR_NAME" >/dev/null
echo "created $INSTANCE_NAME in $AZ; waiting for running state..."

for _ in $(seq 1 60); do
  STATE=$(aws lightsail get-instance-state --region "$REGION" --instance-name "$INSTANCE_NAME" \
    --query 'state.name' --output text 2>/dev/null || echo pending)
  [[ "$STATE" == "running" ]] && break
  sleep 5
done
[[ "${STATE:-}" == "running" ]] || { echo "instance did not reach running state" >&2; exit 1; }

echo "== Firewall: SSH only =="
CIDR="0.0.0.0/0"
if [[ "$RESTRICT_SSH_TO_MY_IP" == "true" ]]; then
  MYIP=$(curl -fsS https://checkip.amazonaws.com | tr -d '\n')
  CIDR="${MYIP}/32"
fi
aws lightsail put-instance-public-ports --region "$REGION" \
  --instance-name "$INSTANCE_NAME" \
  --port-infos "fromPort=22,toPort=22,protocol=tcp,cidrs=${CIDR}" >/dev/null
echo "open ports replaced with: 22/tcp from ${CIDR} (everything else closed)"

IP=$(aws lightsail get-instance --region "$REGION" --instance-name "$INSTANCE_NAME" \
  --query 'instance.publicIpAddress' --output text)
echo
echo "== Done =="
echo "Instance:  $INSTANCE_NAME ($REGION, $STATE)"
echo "Public IP: $IP"
echo "SSH:       ssh -i ${SSH_PUBKEY%.pub} ubuntu@${IP}"
echo
echo "Teardown when finished (stops billing):"
echo "  AWS_REGION=$REGION bash scripts/aws-lightsail/teardown.sh"
