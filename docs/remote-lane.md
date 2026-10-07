# Embedding on another machine's GPU

The `remote` lane sends a run's chunks to `semlith worker` on another machine
and takes the vectors back. It exists so a machine without a GPU, Semlith
Cloud's indexer first of all, can embed on one it rents, and so that the
machine it rents can be shown, before anything is sent, to be a confidential
VM whose GPU is in confidential-computing mode.

Everything else stays where it was: reading, chunking, tokenising, the writer
and the store. The worker receives token ids, returns vectors, opens no store
and writes nothing to disk. Its log carries counts and timings, never content.

## How a connection is trusted

1. **The channel.** The worker makes a key when it starts and never writes it
   down, and serves TLS 1.3 under it. The client accepts whatever certificate
   the worker shows during the handshake (the handshake still proves the worker
   holds the key) and remembers it.
2. **The token.** The client's first frame carries the token the worker was
   started with (`SEMLITH_WORKER_TOKEN`), so nobody else can use the GPU, and a
   fresh 32-byte nonce.
3. **The evidence.** The worker runs its two attestation commands with
   `{binding}` replaced by `sha256(nonce || its certificate)` and answers with
   what they print: on a Google Cloud Confidential VM, a Google Cloud
   Attestation token from `gotpm token --custom-nonce {binding}`, and NVIDIA's
   attestation token from `nvattest attest --nonce {binding} --verifier
   remote`.
4. **The check.** The client verifies each token against the policy it was
   given: the issuer's signature (keys fetched from the issuer over TLS, or
   pinned in the policy), issuer, expiry, audience, every claim the policy
   requires, and that the token's `eat_nonce` is this connection's binding. A
   token made for another nonce or another key is a replay and is refused like
   a forged one.
5. **Only then** does a batch leave the machine. Every reconnect is attested
   afresh. A refusal says what failed (`semlith doctor --gpu`, `semlith accel
   status`, the portal's lane row), and the run carries on with the local
   lanes.

What the Google token proves on a G4 (`hwmodel: GCP_AMD_SEV`): the VM is a
Confidential VM with Secure Boot, in the named project, running as the named
service account. Google's G4 machines offer AMD SEV, not SEV-SNP or TDX
(`--confidential-compute-type=SEV_SNP` is refused for `g4-standard-48`), so
the CPU side rests on Google's vTPM rather than an AMD-signed report, and the
token does not name the software the VM booted. The GPU side is attested by
NVIDIA independently of Google.

## When the worker goes away

A spot VM preempted, a network gone or a worker killed: every batch the lane
held goes back to the run, and whichever lane is left embeds it. Vectors reach
the writer only whole, so the store ends exactly as a run without the remote
lane would have left it. Measured on a G4 stopped 25 s into a 28,676-chunk run:
15,286 chunks were embedded remotely and 13,390 on the CPU, and the run
finished with every chunk.

## Setting it up

On the worker's machine, with the lane it serves working (`semlith doctor
--gpu`):

```sh
SEMLITH_WORKER_TOKEN=$(cat token) semlith worker --listen 0.0.0.0:7400 --lane cuda \
  --attest-cpu 'gotpm token --custom-nonce {binding} --audience semlith-remote-lane' \
  --attest-gpu 'nvattest attest --device gpu --verifier remote --nonce {binding} --format json | jq -r ".detached_eat[0][1]"'
```

The worker checks its lane once at start and refuses to serve one that fails.

On the machine that indexes:

```sh
semlith accel on remote --endpoint host:7400 --token-file token --policy policy.json
```

`SEMLITH_REMOTE_ENDPOINT`, `SEMLITH_REMOTE_TOKEN` and `SEMLITH_REMOTE_POLICY`
take precedence over the saved settings. A policy that checks nothing is
refused; `{"off": true}` runs without attestation, for a worker on a machine
you control yourself, and every place the lane is shown says so.

`scripts/remote-worker.sh` does all of it for a Google Cloud Confidential G4
(RTX PRO 6000, CC on, spot) reached through IAP: `up`, `tunnel`, `use`,
`down`. It writes the policy:

```json
{
  "cpu": {
    "issuer": "https://confidentialcomputing.googleapis.com",
    "jwks_url": "https://www.googleapis.com/service_accounts/v1/metadata/jwk/signer@confidentialspace-sign.iam.gserviceaccount.com",
    "nonce_claim": "eat_nonce",
    "audience": "semlith-remote-lane",
    "require": { "hwmodel": "GCP_AMD_SEV", "secboot": true, "swname": "GCE",
                 "submods.gce.project_id": "<project>",
                 "google_service_accounts": ["<worker service account>"] }
  },
  "gpu": {
    "issuer": "https://nras.attestation.nvidia.com",
    "jwks_url": "https://nras.attestation.nvidia.com/.well-known/jwks.json",
    "nonce_claim": "eat_nonce",
    "require": { "x-nvidia-overall-att-result": true }
  }
}
```

## Measured (2026-10-07)

GCP Confidential G4 (`g4-standard-48`, RTX PRO 6000 Blackwell Server Edition,
CC on, AMD SEV, spot, asia-south1-c), reached from an M1 laptop in India over
SSH on IAP:

| | |
|---|---|
| VM create to GPU attested and ready | 4 min 9 s (driver install and one reboot included) |
| Evidence per connection (both commands) | 3.7–5.0 s |
| `doctor --gpu` through the remote lane | CUDA, cosine 1.0000 on all 32 fixture chunks |
| The semlith repository, 7,169 chunks | 35.2 s on the remote lane; 552.7 s on the M1's CPU |
| Four copies of it, 28,676 chunks | 89.5 s, about 320 chunks/s end to end |

The laptop's link and the tunnel, not the GPU, set that rate; a client in the
same region as the worker sees more.

## Your own NVIDIA GPU (Enterprise)

A machine with its own NVIDIA card needs no remote lane: the `cuda` and `trt`
lanes run on it directly.

1. Linux x86_64 with the NVIDIA driver (580 or later was measured) and the
   Linux release build of semlith, which loads ONNX Runtime from beside the
   binary.
2. `semlith accel on cuda` fetches the CUDA pack (about 1.9 GB) once.
   `semlith accel on trt` fetches TensorRT for RTX; it takes RTX-class cards,
   and a data-centre card is refused with the reason and runs on CUDA instead.
3. `semlith doctor --gpu` embeds the 32 fixture chunks on each lane and
   compares them with the CPU's fp32 vectors. On the RTX PRO 6000 with CC on,
   both CUDA and TensorRT gave cosine 1.0000 on every chunk.

Both lanes are marked experimental: built and checked in CI without the
hardware, and measured on the cards named here.
