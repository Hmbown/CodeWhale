#!/usr/bin/env bash
# EXPERIMENTAL — tears down the CodeWhale Lightsail smoke lab and verifies
# nothing billable is left behind (instance, key pair, static IPs, disks,
# snapshots). Safe to re-run; prints what it finds before deleting.
set -euo pipefail

INSTANCE_NAME="${INSTANCE_NAME:-codewhale-smoke}"
KEY_PAIR_NAME="${KEY_PAIR_NAME:-${INSTANCE_NAME}-key}"
REGION="${AWS_REGION:-$(aws configure get region || true)}"
[[ -n "${REGION}" ]] || { echo "Set AWS_REGION" >&2; exit 1; }

echo "== Current Lightsail resources in ${REGION} =="
aws lightsail get-instances --region "$REGION" \
  --query 'instances[].[name,state.name,bundleId]' --output table || true

if aws lightsail get-instance --region "$REGION" --instance-name "$INSTANCE_NAME" >/dev/null 2>&1; then
  read -r -p "Delete instance '${INSTANCE_NAME}'? Type 'yes': " CONFIRM
  [[ "$CONFIRM" == "yes" ]] || { echo "Aborted."; exit 1; }
  aws lightsail delete-instance --region "$REGION" --instance-name "$INSTANCE_NAME" >/dev/null
  echo "deleted instance ${INSTANCE_NAME}"
else
  echo "instance ${INSTANCE_NAME} not found (already deleted?)"
fi

if aws lightsail get-key-pair --region "$REGION" --key-pair-name "$KEY_PAIR_NAME" >/dev/null 2>&1; then
  aws lightsail delete-key-pair --region "$REGION" --key-pair-name "$KEY_PAIR_NAME" >/dev/null
  echo "deleted key pair ${KEY_PAIR_NAME}"
fi

echo "== Leftover billable resources check =="
echo "-- static IPs (billed when unattached):"
aws lightsail get-static-ips --region "$REGION" --query 'staticIps[].[name,isAttached]' --output table
echo "-- extra disks:"
aws lightsail get-disks --region "$REGION" --query 'disks[].[name,state]' --output table
echo "-- instance snapshots:"
aws lightsail get-instance-snapshots --region "$REGION" --query 'instanceSnapshots[].[name,state]' --output table
echo "-- remaining instances:"
aws lightsail get-instances --region "$REGION" --query 'instances[].[name,state.name]' --output table
echo
echo "If all tables above are empty, Lightsail billing for this lab is fully stopped."
