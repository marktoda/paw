#!/usr/bin/env bash
# Signs the unsigned device build from a GitHub run (the `device` job's
# Release build, without the simulated key, the fixture or the debug
# harnesses) and installs it on a device over Wi-Fi. Runs on Linux; the
# signing key stays on this machine.
# Usage: install.sh [iphone|ipad] [run-id]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
set -a; . "$root/.env"; set +a
export SSL_CERT_FILE="${SSL_CERT_FILE:-/etc/ssl/certs/ca-certificates.crt}"

device="${1:-iphone}"
case "$device" in
  iphone) udid="$IPHONE_UDID"; host="${IPHONE_HOST:-}" ;;
  ipad) udid="$IPAD_UDID"; host="${IPAD_HOST:-}" ;;
  *) echo "usage: install.sh [iphone|ipad] [run-id]"; exit 2 ;;
esac
cd "$root"
run="${2:-$(gh run list --limit 1 --json databaseId -q '.[0].databaseId')}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gh run download "$run" --name device-app --dir "$work"
(cd "$work" && unzip -q Paw.app.zip)
app="$work/Paw.app"
appex="PlugIns/PawAutoFill.appex"
# What Settings › About will say, e.g. "build 1a2b3c4 Release".
python3 -I -c 'import plistlib, sys
info = plistlib.load(open(sys.argv[1], "rb"))
print("build", info["PawCommit"], info["PawConfiguration"])' "$app/Info.plist"

# The team ID and full entitlements come from the provisioning profiles.
entitlements() {
  python3 -I - "$1" <<'PY'
import plistlib, sys
raw = open(sys.argv[1], "rb").read()
start, end = raw.index(b"<?xml"), raw.index(b"</plist>") + len(b"</plist>")
granted = plistlib.loads(raw[start:end])["Entitlements"]
wanted = dict(granted)
# The profile grants a wildcard keychain group; the signature must name one.
wanted["keychain-access-groups"] = [granted["application-identifier"]] + [
    g for g in granted["keychain-access-groups"] if "*" not in g]  # keeps com.apple.token
sys.stdout.buffer.write(plistlib.dumps(wanted))
PY
}
entitlements "$PAW_SIGNING_DIR/Paw.mobileprovision" > "$work/Paw.ent"
entitlements "$PAW_SIGNING_DIR/PawAutoFill.mobileprovision" > "$work/PawAutoFill.ent"
cp "$PAW_SIGNING_DIR/Paw.mobileprovision" "$app/embedded.mobileprovision"
cp "$PAW_SIGNING_DIR/PawAutoFill.mobileprovision" "$app/$appex/embedded.mobileprovision"

openssl x509 -inform DER -in "$PAW_SIGNING_DIR/dev.cer" -out "$work/dev.pem" 2>/dev/null \
  || cp "$PAW_SIGNING_DIR/dev.cer" "$work/dev.pem"

if ! rcodesign sign \
    --pem-file "$PAW_SIGNING_DIR/dev.key" --pem-file "$work/dev.pem" \
    -e "$work/Paw.ent" -e "$appex:$work/PawAutoFill.ent" \
    "$app" > "$work/sign.log" 2>&1; then
  cat "$work/sign.log" >&2
  echo "install: signing failed, so nothing was installed" >&2
  exit 1
fi
grep -E "error|warning|signing bundle" "$work/sign.log" || true

target=(--udid "$udid")
[ -n "$host" ] && target=(--mobdev2-host "$host")
uvx pymobiledevice3 apps install "${target[@]}" "$app"
echo "installed run $run on $device"
