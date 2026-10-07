#!/usr/bin/env bash
# A `semlith worker` on a Google Cloud Confidential G4 (RTX PRO 6000, CC on,
# AMD SEV), reached through an IAP tunnel, for the remote embedding lane.
#
#   scripts/remote-worker.sh up      create the VM, install semlith, start the worker
#   scripts/remote-worker.sh tunnel  forward localhost:7400 to it (runs until stopped)
#   scripts/remote-worker.sh use     point this machine's remote lane at the tunnel
#   scripts/remote-worker.sh down    delete the VM (always, at the end of a session)
#
# Settings from the environment: PROJECT (semlith-cloud), ZONE (asia-south1-c),
# VM (sml-g4-worker), SEMLITH_VERSION (a release tag, or "local" to ship this
# checkout and build it there), SPOT (1). The worker's token is made here and
# kept in $STATE; the attestation policy is written beside it.
#
# The VM has no public address (the project forbids one): the worker is
# reached only through IAP, and fetches what it needs through Cloud NAT. Spot
# capacity may be absent; `up` says so and nothing is left behind.
set -euo pipefail

PROJECT=${PROJECT:-semlith-cloud}
ZONE=${ZONE:-asia-south1-c}
REGION=${ZONE%-*}
VM=${VM:-sml-g4-worker}
PORT=7400
SA=sml-worker@$PROJECT.iam.gserviceaccount.com
STATE=${STATE:-$HOME/.semlith/remote-worker/$VM}
AUDIENCE=semlith-remote-lane
here=$(cd "$(dirname "$0")/.." && pwd)
g() { gcloud --project "$PROJECT" --quiet "$@"; }
on_vm() { g compute ssh "$VM" --zone "$ZONE" --tunnel-through-iap --command "$1"; }

# Run once per project: the service account the attestation token names, NAT
# for the downloads, and IAP's range allowed to the worker's port.
prepare() {
  g services enable compute.googleapis.com confidentialcomputing.googleapis.com iap.googleapis.com >/dev/null
  g iam service-accounts describe "$SA" >/dev/null 2>&1 ||
    g iam service-accounts create "${SA%%@*}" --display-name "semlith remote worker (attestation only)"
  g projects add-iam-policy-binding "$PROJECT" --member "serviceAccount:$SA" \
    --role roles/confidentialcomputing.workloadUser --condition=None >/dev/null
  g compute routers describe sml-nat-router --region "$REGION" >/dev/null 2>&1 ||
    g compute routers create sml-nat-router --network default --region "$REGION"
  g compute routers nats describe sml-nat --router sml-nat-router --region "$REGION" >/dev/null 2>&1 ||
    g compute routers nats create sml-nat --router sml-nat-router --region "$REGION" \
      --auto-allocate-nat-external-ips --nat-all-subnet-ip-ranges
  g compute firewall-rules describe sml-iap-worker >/dev/null 2>&1 ||
    g compute firewall-rules create sml-iap-worker --network default --direction INGRESS \
      --source-ranges 35.235.240.0/20 --allow "tcp:22,tcp:$PORT" --target-tags sml-worker
}

up() {
  mkdir -p "$STATE" && chmod 700 "$STATE"
  prepare
  local spot=()
  [ "${SPOT:-1}" = 1 ] && spot=(--provisioning-model=SPOT --instance-termination-action=DELETE)
  g compute instances create "$VM" --zone "$ZONE" --machine-type g4-standard-48 \
    --confidential-compute-type=SEV --maintenance-policy=TERMINATE "${spot[@]}" \
    --image-project=ubuntu-os-cloud --image-family=ubuntu-2404-lts-amd64 --boot-disk-size=100G \
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring --no-address \
    --tags sml-worker --service-account "$SA" --scopes cloud-platform \
    --metadata-from-file startup-script="$here/scripts/remote-worker-boot.sh"
  echo "waiting for the driver, the GPU's attestation and its ready state (about 5 minutes)"
  local deadline=$((SECONDS + 1200))
  until on_vm 'grep -q ": ready" /var/log/sml-ready.txt' 2>/dev/null; do
    [ $SECONDS -lt $deadline ] || { echo "the GPU never reached ready; see /var/log/sml-gpu-attest.json"; exit 1; }
    sleep 20
  done
  install_semlith
  head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$STATE/token"
  chmod 600 "$STATE/token"
  g compute scp "$STATE/token" "$VM:/tmp/token" --zone "$ZONE" --tunnel-through-iap
  # Root, because the vTPM is root's. ponytail: the worker runs as root on a
  # single-purpose VM; a tss-group user if the VM ever does anything else.
  on_vm "sudo install -m 600 /tmp/token /root/worker-token && rm /tmp/token && sudo bash -c '
    SEMLITH_WORKER_TOKEN=\$(cat /root/worker-token) nohup /usr/local/bin/semlith worker \
      --listen 0.0.0.0:$PORT --lane ${LANE:-cuda} \
      --attest-cpu \"gotpm token --custom-nonce {binding} --audience $AUDIENCE\" \
      --attest-gpu \"nvattest attest --device gpu --verifier remote --nonce {binding} --format json | jq -r .detached_eat[0][1]\" \
      > /var/log/sml-worker.log 2>&1 &'"
  policy
  echo "worker up on $VM; next: $0 tunnel (in another terminal), then $0 use"
}

install_semlith() {
  if [ "${SEMLITH_VERSION:-local}" = local ]; then
    (cd "$here" && git archive --format=tar.gz -o /tmp/semlith-src.tgz HEAD)
    g compute scp /tmp/semlith-src.tgz "$VM:/tmp/src.tgz" --zone "$ZONE" --tunnel-through-iap
    on_vm 'sudo bash -c "rm -rf /opt/sml && mkdir -p /opt/sml && cd /opt/sml && tar xzf /tmp/src.tgz &&
      source /root/.cargo/env && cargo build --release -q && install target/release/semlith /usr/local/bin/"'
  else
    on_vm "curl -fsSL https://raw.githubusercontent.com/semlith/semlith/main/install.sh |
      sudo SEMLITH_VERSION=$SEMLITH_VERSION SEMLITH_INSTALL_DIR=/usr/local/bin sh"
  fi
  on_vm "sudo semlith accel on ${LANE:-cuda}"
}

# What this machine trusts the worker to be: Google's Confidential VM token
# for a SEV VM with Secure Boot in this project under the worker's service
# account, and NVIDIA's token saying the GPU passed.
policy() {
  cat > "$STATE/policy.json" <<EOF
{
  "cpu": {
    "issuer": "https://confidentialcomputing.googleapis.com",
    "jwks_url": "https://www.googleapis.com/service_accounts/v1/metadata/jwk/signer@confidentialspace-sign.iam.gserviceaccount.com",
    "nonce_claim": "eat_nonce",
    "audience": "$AUDIENCE",
    "require": {
      "hwmodel": "GCP_AMD_SEV",
      "secboot": true,
      "swname": "GCE",
      "submods.gce.project_id": "$PROJECT",
      "google_service_accounts": ["$SA"]
    }
  },
  "gpu": {
    "issuer": "https://nras.attestation.nvidia.com",
    "jwks_url": "https://nras.attestation.nvidia.com/.well-known/jwks.json",
    "nonce_claim": "eat_nonce",
    "require": { "x-nvidia-overall-att-result": true }
  }
}
EOF
  echo "policy: $STATE/policy.json"
}

tunnel() { g compute start-iap-tunnel "$VM" "$PORT" --local-host-port "localhost:$PORT" --zone "$ZONE"; }

use() {
  semlith accel on remote --endpoint "localhost:$PORT" --token-file "$STATE/token" --policy "$STATE/policy.json"
}

down() { g compute instances delete "$VM" --zone "$ZONE" || true; g compute instances list; }

case "${1:-}" in
  up | tunnel | use | down | policy | prepare) "$1" ;;
  *) sed -n '2,15p' "$0"; exit 2 ;;
esac
