#!/bin/bash
# The boot script of a G4 remote worker (scripts/remote-worker.sh up). Phase 1,
# once: NVIDIA's signed open modules (Secure Boot refuses a DKMS build), the
# LKCA modprobe and persistence mode Google's guide asks for, nvattest, gotpm
# and rustup; then a reboot. Every boot after: attest the GPU and set it ready.
set -x
M=/var/lib/sml-phase1
if [ -f $M ]; then
  nvattest attest --device gpu --verifier local > /var/log/sml-gpu-attest.json 2>&1 && nvidia-smi conf-compute -srs 1
  nvidia-smi conf-compute -grs > /var/log/sml-ready.txt 2>&1
  exit 0
fi
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y build-essential pkg-config git jq golang-go libssl-dev curl
apt-get install -y linux-modules-nvidia-580-server-open-$(uname -r) nvidia-headless-no-dkms-580-server-open nvidia-utils-580-server
echo "install nvidia /sbin/modprobe ecdsa_generic; /sbin/modprobe ecdh; /sbin/modprobe --ignore-install nvidia" > /etc/modprobe.d/nvidia-lkca.conf
update-initramfs -u
test -f /usr/lib/systemd/system/nvidia-persistenced.service && sed -i "s/no-persistence-mode/uvm-persistence-mode/g" /usr/lib/systemd/system/nvidia-persistenced.service
systemctl daemon-reload
(cd /tmp && wget -q https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2404/x86_64/cuda-keyring_1.1-1_all.deb && dpkg -i cuda-keyring_1.1-1_all.deb && apt-get update -y && apt-get install -y nvattest)
(cd /opt && git clone --depth 1 https://github.com/google/go-tpm-tools.git && cd go-tpm-tools/cmd/gotpm && HOME=/root GOPATH=/root/go GOCACHE=/root/.cache/go go build -o /usr/local/bin/gotpm .)
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
touch $M
reboot
