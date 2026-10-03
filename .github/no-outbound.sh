#!/usr/bin/env bash
# With no cloud token the binary makes no outbound connection: a packet
# capture of everything one user's processes send anywhere but loopback,
# during a whole session -- index, re-index, search, the daemon, its portal
# and its Privacy route. Linux only; needs sudo for iptables and tcpdump.
#
# The capture is per user, not per host: the runner's own agent talks to
# GitHub the whole time, so semlith runs as a user of its own, and an
# iptables owner match hands that user's packets to NFLOG, which tcpdump
# reads. A curl as the same user first proves the capture sees traffic at
# all; a check that cannot fail is not a check.

set -euo pipefail

bin=$(command -v semlith)
user=nocap
port=7399
sudo useradd -m "$user"
sudo install -m 0755 "$bin" /usr/local/bin/semlith-under-test
# umask 077: Ubuntu's default 002 makes the model cache group-writable, and
# semlith refuses to load weights from a directory others can write.
as() { sudo -u "$user" -H env HOME="/home/$user" SEMLITH_NO_SERVICE=1 sh -c 'umask 077; exec "$@"' sh "$@"; }
uid=$(id -u "$user")
corpus=/home/$user/corpus

as mkdir -p "$corpus"

as sh -c "printf 'pub fn total(items: &[u32]) -> u32 { items.iter().sum() }\n' > $corpus/lib.rs"
as sh -c "printf '# Notes\n\nThe order total is the sum of its items.\n' > $corpus/notes.md"
# The model is fetched here, before the capture: fetching it is the one
# download a first run makes, and it is not what this check is about.
as semlith-under-test index "$corpus" > /dev/null
# semlith made its model cache private itself, whatever the umask.
sudo stat -c '%a %n' "/home/$user/.cache/semlith/models"

capture() {
  sudo iptables -I OUTPUT -m owner --uid-owner "$uid" ! -o lo -j NFLOG --nflog-group 7
  sudo ip6tables -I OUTPUT -m owner --uid-owner "$uid" ! -o lo -j NFLOG --nflog-group 7
  sudo tcpdump -i nflog:7 -nn -U -w "$1" 2> /dev/null &
  tcpdump_pid=$!
  sleep 2
}
release() {
  sleep 2
  sudo kill "$tcpdump_pid" 2> /dev/null || true
  wait "$tcpdump_pid" 2> /dev/null || true
  sudo iptables -D OUTPUT -m owner --uid-owner "$uid" ! -o lo -j NFLOG --nflog-group 7
  sudo ip6tables -D OUTPUT -m owner --uid-owner "$uid" ! -o lo -j NFLOG --nflog-group 7
}
packets() { sudo tcpdump -r "$1" -nn 2> /dev/null | wc -l | tr -d ' '; }

# The control: the same user reaching the internet is seen.
capture control.pcap
as curl -s -o /dev/null -m 10 https://github.com || true
release
control=$(packets control.pcap)
echo "control: $control packets from a curl as $user"
[ "$control" -gt 0 ] || { echo "the capture saw nothing of a real request; it cannot prove an absence"; exit 1; }

# The session.
capture session.pcap
step() { echo "-- $*"; }
step "re-index after an edit"
as sh -c "printf 'Totals round half up.\n' >> $corpus/notes.md"
timeout 300 sudo -u "$user" -H env HOME="/home/$user" SEMLITH_NO_SERVICE=1 sh -c 'umask 077; exec "$@"' sh semlith-under-test index "$corpus" > /dev/null
step "search"
timeout 120 sudo -u "$user" -H env HOME="/home/$user" sh -c "umask 077; cd $corpus && exec semlith-under-test search 'where is the order total'" > /dev/null
# The daemon as a transient systemd service: started detached, holding no
# terminal (a daemon backgrounded under sudo kept sudo waiting until the
# job's limit), and still running as the capture user.
step "the daemon and its portal"
sudo systemd-run --quiet --unit=semlith-nocap --uid="$user" --gid="$user" \
  --property=UMask=0077 --working-directory="/home/$user" \
  --setenv=HOME="/home/$user" --setenv=SEMLITH_NO_SERVICE=1 \
  --property=StandardOutput="file:/home/$user/daemon.out" --property=StandardError="file:/home/$user/daemon.out" \
  /usr/local/bin/semlith-under-test start --port "$port" --no-service
for _ in $(seq 1 90); do sudo grep -q 'token=' "/home/$user/daemon.out" 2> /dev/null && break; sleep 1; done
token=$(sudo grep -oE 'token=[0-9a-f]+' "/home/$user/daemon.out" | head -1 | cut -d= -f2)
[ -n "$token" ] || { echo "the daemon printed no token:"; sudo cat "/home/$user/daemon.out"; exit 1; }
curl -fsS -m 30 -o /dev/null "http://127.0.0.1:$port/"
curl -fsS -m 30 -H "Semlith-Token: $token" "http://127.0.0.1:$port/api/privacy" > privacy.json
curl -fsS -m 60 -H "Semlith-Token: $token" "http://127.0.0.1:$port/api/search?q=order%20total" > /dev/null
step "stop"
sudo systemctl stop semlith-nocap 2> /dev/null || true
sudo pkill -u "$user" -f semlith-under-test || true
release
session=$(packets session.pcap)
echo "session: $session packets from semlith to anywhere but loopback"
sudo tcpdump -r session.pcap -nn 2> /dev/null | head -20
python3 - <<'PY'
import json
p = json.load(open("privacy.json"))
print("privacy route:", json.dumps({k: p[k] for k in p if k in ("outbound", "connections", "sockets")})[:400])
PY
[ "$session" -eq 0 ]
