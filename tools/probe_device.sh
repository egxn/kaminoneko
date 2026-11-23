#!/usr/bin/env bash
# First-contact probe of an unknown Wi-Fi device: ports, banners, web page, FTP
# and ESC/POS tests. Everything is saved to captures/probe_<date>/.
#
# Usage (already connected to the device's Wi-Fi):
#   tools/probe_device.sh            # target = gateway of the current network
#   tools/probe_device.sh 192.168.1.1
#
# Optional environment:
#   CAMERA_WIFI="Children's camera d109"  switch to this Wi-Fi first and come back afterwards
#   RETURN_TO="My Wi-Fi"                  Wi-Fi to reconnect to when done (if CAMERA_WIFI is unset)
#   NO_PRINT=1                            do not send ESC/POS test prints
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/captures/probe_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$OUT"
exec > >(tee -a "$OUT/summary.log") 2>&1

section() { printf '\n===== %s =====\n' "$*"; }

RETURN_TO="${RETURN_TO-}"
if [ -n "${CAMERA_WIFI-}" ]; then
  RETURN_TO=$(nmcli -t -f NAME,TYPE con show --active | awk -F: '$2=="802-11-wireless"{print $1; exit}')
  echo "Switching to '$CAMERA_WIFI' (will return to '$RETURN_TO')..."
  nmcli con up id "$CAMERA_WIFI" || { echo "Could not connect to '$CAMERA_WIFI'"; exit 1; }
  sleep 3
fi

reconnect() {
  if [ -n "$RETURN_TO" ]; then
    section "Reconnecting to '$RETURN_TO'"
    nmcli con up id "$RETURN_TO" || echo "Could not reconnect; do it manually."
  fi
  echo
  echo "Results saved to: $OUT"
}
trap reconnect EXIT

GW=$(ip route | awk '/^default/ {print $3; exit}')
IP="${1:-${GW:-192.168.1.1}}"

section "Current network"
nmcli -t -f ACTIVE,SSID,BSSID,CHAN,SIGNAL,SECURITY dev wifi | grep '^yes'
ip -4 addr show | grep -E 'inet |^[0-9]'
ip route
echo "Target: $IP"

if ! timeout 3 bash -c "</dev/tcp/$IP/80" 2>/dev/null &&
   ! timeout 3 bash -c "</dev/tcp/$IP/21" 2>/dev/null; then
  echo "!! $IP does not answer on 80 or 21. Are you connected to the device's Wi-Fi?"
  exit 1
fi

section "Device MAC (identifies the Wi-Fi module vendor)"
ping -c 2 -W 2 "$IP" >/dev/null 2>&1
ip neigh show "$IP"

section "Other devices on the network"
nmap -sn -n "${IP%.*}.0/24" | grep -E 'report|MAC'

section "Scan of ALL TCP ports"
# Note: cheap devices can get overwhelmed by a fast full scan and report everything as filtered.
nmap -Pn -n -p- -T4 --min-rate 2000 -oN "$OUT/nmap_all.txt" -oG "$OUT/nmap_all.gnmap" "$IP" | grep -E 'open|PORT'
PORTS=$(grep -oE '[0-9]+/open/tcp' "$OUT/nmap_all.gnmap" | cut -d/ -f1 | paste -sd, -)
[ -z "$PORTS" ] && PORTS="21,80,5000,5001,5002,7070"
echo "Open ports: $PORTS"

section "Service versions + default scripts"
nmap -Pn -n -sV -sC -p "$PORTS" -oN "$OUT/nmap_versions.txt" "$IP"

if [ "$EUID" -eq 0 ]; then
  section "UDP (top 50)"
  nmap -Pn -n -sU --top-ports 50 -oN "$OUT/nmap_udp.txt" "$IP"
fi

section "SSDP/UPnP discovery"
python3 -I - <<'EOF' | tee "$OUT/ssdp.txt"
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(4)
s.sendto(b'M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\n'
         b'MAN: "ssdp:discover"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n',
         ('239.255.255.250', 1900))
try:
    while True:
        d, a = s.recvfrom(4096)
        print(a, d.decode(errors='replace'))
except socket.timeout:
    print("(end)")
EOF

section "HTTP on every port"
for p in ${PORTS//,/ }; do
  [ "$p" = 21 ] && continue
  echo "--- http://$IP:$p/ ---"
  curl -s -i -m 5 "http://$IP:$p/" > "$OUT/http_$p.txt"
  head -c 600 "$OUT/http_$p.txt"; echo
done
for path in index.html status info config api cgi-bin/ device.json; do
  code=$(curl -s -o "$OUT/http80_${path//\//_}.txt" -w '%{http_code}' -m 5 "http://$IP/$path")
  echo "/$path -> $code"
done

section "Mirror of the web page (port 80)"
timeout 120 wget -q -r -l 3 -np -e robots=off --quota=20m -P "$OUT/web" "http://$IP/"
find "$OUT/web" -type f 2>/dev/null | head -50

section "Anonymous FTP"
curl -s -m 10 "ftp://$IP/" --user anonymous:anonymous | tee "$OUT/ftp_root.txt"
if [ -s "$OUT/ftp_root.txt" ]; then
  echo "Anonymous FTP works, copying (max 50 MB / 3 min)..."
  timeout 180 wget -q -r -l 4 --quota=50m -P "$OUT/ftp" "ftp://anonymous:anonymous@$IP/"
  find "$OUT/ftp" -type f 2>/dev/null | head -100
fi

section "Banners and non-printing ESC/POS queries"
for p in ${PORTS//,/ }; do
  [ "$p" = 21 ] || [ "$p" = 80 ] && continue
  echo "--- port $p: banner (nothing sent) ---"
  timeout 6 nc -w 4 "$IP" "$p" </dev/null | head -c 512 | xxd
  echo "--- port $p: ESC/POS status (DLE EOT 1) ---"
  printf '\x10\x04\x01' | timeout 6 nc -w 3 "$IP" "$p" | head -c 512 | xxd
  echo "--- port $p: printer ID (GS I 1) ---"
  printf '\x1d\x49\x01' | timeout 6 nc -w 3 "$IP" "$p" | head -c 512 | xxd
done

if [ "${NO_PRINT:-0}" != 1 ]; then
  section "ESC/POS print tests (watch for paper)"
  for p in ${PORTS//,/ }; do
    [ "$p" = 21 ] || [ "$p" = 80 ] && continue
    echo ">>> Sending 'TEST PORT $p'..."
    printf '\x1b\x40TEST PORT %s\n\n\n\n' "$p" | timeout 6 nc -N -w 3 "$IP" "$p" | head -c 512 | xxd
    sleep 4
  done
fi

section "Done"
echo "If any paper came out, note which port it said."
