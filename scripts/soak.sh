#!/usr/bin/env bash
# Soak test for correntd (docs/SERVER-STABILITY.md): runs a scratch server on loopback for a
# while under a steady mix of API traffic, library scans and (when libmpv is present)
# transcodes, sampling its memory, threads, open files and /metrics into a CSV.
#
#   scripts/soak.sh [minutes=10] [scratch dir=target/soak]
#   SOAK_TORRENTS=5000 scripts/soak.sh 4320    # the 72 h run: also 5k torrents (half paused)
#
# Pass: the server answers throughout, RSS and open files level off, no task failures, no
# panics in the log. Uses only generated media; deletes nothing outside the scratch dir.
# Runs on Linux and in Git Bash on Windows (there the samples come from Get-Process, and the
# fds column counts handles).
set -euo pipefail
cd "$(dirname "$0")/.."
# Windows: the Store stub answers to python3 without being Python; the server wants E:/ paths.
py=python3; python3 -c '' 2>/dev/null || py=python
here=$(pwd -W 2>/dev/null || pwd)
exe=; [ -e /proc/self/winpid ] && exe=.exe
minutes=${1:-10}
dir=${2:-target/soak}
port=18999
rm -rf "$dir" && mkdir -p "$dir/data" "$dir/media/Photos" "$dir/media/Movies"
torrents=${SOAK_TORRENTS:-0}
cargo build --release -p correntd -p corrent-mpv --bin correntd --bin corrent-mpvw >/dev/null
# A copy, so builds during a long run don't touch the running binary.
mkdir -p "$dir/bin" && cp target/release/correntd$exe target/release/corrent-mpvw$exe "$dir/bin/"
bin=$dir/bin/correntd$exe
cat > "$dir/data/settings.toml" <<TOML
[connection]
upnp = false
listen_address = "127.0.0.1"
random_port = true
[bittorrent]
dht = false
lsd = false
TOML
printf 'metrics = true\ndiscovery = false\n' > "$dir/data/server.toml"
# Generated media: photos always, a short video when mpv is installed.
"$py" - "$dir/media/Photos" <<'PY'
import sys, zlib, struct
def png(path, rgb):
    w, h = 64, 48
    raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    open(path, 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
for i in range(200):
    png(f"{sys.argv[1]}/photo{i:03}.png", ((i * 37) % 256, (i * 91) % 256, (i * 13) % 256))
PY
if command -v mpv >/dev/null; then
  mkdir -p "$dir/media/Movies/Soak Test (2020)"
  mpv --really-quiet "av://lavfi:testsrc=duration=60:size=1280x720:rate=24" --o="$dir/media/Movies/Soak Test (2020)/Soak Test (2020).mkv" --of=matroska --ovc=libx264 --ovcopts=preset=ultrafast >/dev/null 2>&1 || true
fi
CORRENT_ADMIN_PASSWORD=soak-password-1 "$bin" --data-dir "$dir/data" --init-admin admin >/dev/null
"$bin" --data-dir "$dir/data" --bind 127.0.0.1 --https-port $port --http-port 0 --no-tls > "$dir/server.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
B=http://127.0.0.1:$port/api/v1
for _ in $(seq 50); do curl -sf "$B/server/info" >/dev/null && break; sleep 0.2; done
TOK=$(curl -sf -X POST "$B/auth/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"soak-password-1","device":{"name":"soak","platform":"linux"}}' | "$py" -c 'import sys,json;print(json.load(sys.stdin)["token"])')
api() { curl -sf -H "authorization: Bearer $TOK" -H 'content-type: application/json' "$@"; }
api -X POST "$B/libraries" -d "{\"name\":\"Photos\",\"kind\":\"photos\",\"folders\":[\"$here/$dir/media/Photos\"]}" >/dev/null
api -X POST "$B/libraries" -d "{\"name\":\"Movies\",\"kind\":\"movies\",\"folders\":[\"$here/$dir/media/Movies\"]}" >/dev/null
if [ "$torrents" -gt 0 ]; then
  # Synthetic single-file torrents (16 KiB, random piece hashes; no peers: DHT, LSD and UPnP are
  # off and there are no trackers), added 100 per request; every other batch paused.
  mkdir -p "$dir/torrents" "$dir/downloads"
  "$py" - "$dir/torrents" "$torrents" <<'PY'
import sys, os
def b(x):
    if isinstance(x, int): return b'i%de' % x
    if isinstance(x, bytes): return b'%d:%s' % (len(x), x)
    if isinstance(x, str): return b(x.encode())
    if isinstance(x, dict): return b'd' + b''.join(b(k) + b(v) for k, v in sorted(x.items())) + b'e'
for i in range(int(sys.argv[2])):
    info = {'name': f'soak-{i:05}.bin', 'piece length': 16384, 'length': 16384, 'pieces': os.urandom(20)}
    open(f"{sys.argv[1]}/{i:05}.torrent", 'wb').write(b({'info': info}))
PY
  ls "$dir/torrents" | split -l 100 - "$dir/batch-"
  n=0
  for f in "$dir"/batch-*; do
    n=$((n + 1)); paused=$([ $((n % 2)) -eq 0 ] && echo true || echo false)
    args=(-F "options={\"paused\":$paused,\"save_path\":\"$here/$dir/downloads\"}")
    while read -r t; do args+=(-F "file=@$dir/torrents/$t;filename=$t"); done < "$f"
    curl -sf -H "authorization: Bearer $TOK" "${args[@]}" "$B/torrents" >/dev/null || { echo "adding torrents failed"; exit 1; }
  done
  rm -f "$dir"/batch-*
  # Wait until the engine has them all (up to 2 h), reporting the rate.
  added_at=$(date +%s)
  for _ in $(seq 1440); do
    have=$(api "$B/torrents" | "$py" -c 'import sys,json;print(len(json.load(sys.stdin)["torrents"]))')
    [ "$have" -ge "$torrents" ] && break; sleep 5
  done
  echo "torrents: $have after $(( $(date +%s) - added_at )) s"
fi
csv="$dir/samples.csv"
echo "t_secs,rss_kib,threads,fds,ws_clients,transcodes,errors" > "$csv"
start=$(date +%s); end=$((start + minutes * 60)); errors=0; round=0
while [ "$(date +%s)" -lt "$end" ]; do
  round=$((round + 1))
  for path in home "items?limit=50" "library/search?q=photo" libraries torrents tasks; do
    api "$B/$path" >/dev/null || errors=$((errors + 1))
  done
  if [ $((round % 20)) -eq 0 ]; then
    api -X POST "$B/tasks/scan_libraries/run" >/dev/null || errors=$((errors + 1))
  fi
  if [ $((round % 15)) -eq 0 ]; then
    movie=$(api "$B/items?kind=movie&limit=1" | "$py" -c 'import sys,json;i=json.load(sys.stdin)["items"];print(i[0]["id"] if i else "")')
    if [ -n "$movie" ]; then
      url=$(api -X POST "$B/playback/$movie" -d '{"profile":"web","max_height":480,"force_transcode":true}' | "$py" -c 'import sys,json;print(json.load(sys.stdin).get("url",""))' || true)
      if [ -n "$url" ]; then
        # The playlist, then its first segment (that is what makes the encoder run).
        seg=$(curl -sf -H "authorization: Bearer $TOK" "http://127.0.0.1:$port$url" | grep -v '^#' | head -1 || true)
        [ -n "$seg" ] && { curl -sf -H "authorization: Bearer $TOK" "http://127.0.0.1:$port${url%/*}/$seg" >/dev/null || errors=$((errors + 1)); }
      fi
    fi
  fi
  if [ $((round % 10)) -eq 0 ]; then
    t=$(( $(date +%s) - start ))
    if [ -e /proc/$pid/winpid ]; then
      IFS=, read -r rss thr fds < <(powershell -NoProfile -Command "\$p = Get-Process -Id $(cat /proc/$pid/winpid); '{0},{1},{2}' -f [int](\$p.WorkingSet64 / 1KB), \$p.Threads.Count, \$p.HandleCount" | tr -d '\r')
    else
      rss=$(awk '/VmRSS/ {print $2}' /proc/$pid/status)
      thr=$(awk '/Threads/ {print $2}' /proc/$pid/status)
      fds=$(ls /proc/$pid/fd | wc -l)
    fi
    m=$(curl -sf "http://127.0.0.1:$port/metrics" || true)
    ws=$(echo "$m" | awk '/^corrent_realtime_clients/ {print $2}')
    tc=$(echo "$m" | awk '/^corrent_transcode_sessions/ {print $2}')
    echo "$t,$rss,$thr,$fds,${ws:-0},${tc:-0},$errors" >> "$csv"
  fi
  sleep 0.5
done
failed=$(api "$B/tasks" | "$py" -c 'import sys,json;print(sum(1 for t in json.load(sys.stdin) if t.get("last") and t["last"].get("ok") is False))')
panics=$(grep -c 'panicked' "$dir/server.log" || true)
echo "soak: $minutes min, $round rounds, $errors request errors, $failed failed tasks, $panics panics"
echo "samples: $csv"; head -2 "$csv"; tail -1 "$csv"
[ "$errors" -eq 0 ] && [ "$failed" -eq 0 ] && [ "$panics" -eq 0 ]
