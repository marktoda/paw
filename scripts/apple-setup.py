#!/usr/bin/env python3
"""Sets up everything Apple needs to run paw on your own devices, with no Mac:
registers the devices, creates the app IDs with their capabilities, a
development certificate, and one provisioning profile per target.

Safe to run again: it reuses what already exists, and re-creates the profiles
so they include every registered device.

The targets, their bundle identifiers and their capabilities come from the
entitlements in app/project.yml. Reads .env (see .env.example). Run with:
  uv run --with pyjwt --with cryptography --with pyyaml scripts/apple-setup.py
"""
import base64, json, os, pathlib, plistlib, sys, time, urllib.error, urllib.request

import jwt
import yaml
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

ROOT = pathlib.Path(__file__).resolve().parent.parent
for line in (ROOT / ".env").read_text().splitlines():
    if "=" in line and not line.lstrip().startswith("#"):
        k, v = line.split("=", 1)
        os.environ.setdefault(k.strip(), v.strip())

KEY_ID, ISSUER = os.environ["ASC_KEY_ID"], os.environ["ASC_ISSUER_ID"]
KEY = pathlib.Path(os.environ["ASC_KEY_PATH"]).read_text()
SIGNING = pathlib.Path(os.environ["PAW_SIGNING_DIR"])
SIGNING.mkdir(parents=True, exist_ok=True)
SIGNING.chmod(0o700)

CAPABILITY = {
    "com.apple.developer.authentication-services.autofill-credential-provider": "AUTOFILL_CREDENTIAL_PROVIDER",
    "com.apple.security.application-groups": "APP_GROUPS",
}
GROUPS = "com.apple.security.application-groups"


def targets():
    """Target name -> (bundle identifier, entitlements), for what installs on a device."""
    project = yaml.safe_load((ROOT / "app/project.yml").read_text())
    out = {}
    for name, t in project["targets"].items():
        if t["type"] in ("application", "app-extension"):
            entitlements = t["entitlements"]["properties"]
            unknown = set(entitlements) - set(CAPABILITY)
            if unknown:
                sys.exit(f"apple-setup.py does not know the capability for {', '.join(sorted(unknown))}")
            out[name] = (t["settings"]["base"]["PRODUCT_BUNDLE_IDENTIFIER"], entitlements)
    return out


TARGETS = targets()
DEVICES = {name: os.environ[var] for name, var in
           [("iPhone", "IPHONE_UDID"), ("iPad", "IPAD_UDID")] if os.environ.get(var)}


def api(method, path, body=None):
    now = int(time.time())
    token = jwt.encode({"iss": ISSUER, "iat": now, "exp": now + 600, "aud": "appstoreconnect-v1"},
                       KEY, algorithm="ES256", headers={"kid": KEY_ID})
    req = urllib.request.Request(
        "https://api.appstoreconnect.apple.com" + path, method=method,
        data=json.dumps(body).encode() if body else None,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        detail = json.loads(e.read() or b"{}")
        errors = "; ".join(x.get("detail", x.get("title", "")) for x in detail.get("errors", []))
        sys.exit(f"{method} {path} failed ({e.code}): {errors or detail}")


def devices():
    existing = {d["attributes"]["udid"]: d["id"] for d in api("GET", "/v1/devices?limit=200")["data"]}
    ids = []
    for name, udid in DEVICES.items():
        if udid not in existing:
            created = api("POST", "/v1/devices", {"data": {"type": "devices", "attributes": {
                "name": f"paw {name}", "platform": "IOS", "udid": udid}}})
            existing[udid] = created["data"]["id"]
            print(f"registered {name} {udid}")
        ids.append(existing[udid])
    return ids


def bundle_ids():
    existing = {b["attributes"]["identifier"]: b["id"]
                for b in api("GET", "/v1/bundleIds?limit=200")["data"]}
    out = {}
    for target, (identifier, entitlements) in TARGETS.items():
        if identifier not in existing:
            created = api("POST", "/v1/bundleIds", {"data": {"type": "bundleIds", "attributes": {
                "identifier": identifier, "name": f"paw {target}", "platform": "IOS"}}})
            existing[identifier] = created["data"]["id"]
            print(f"created app ID {identifier}")
        bid = existing[identifier]
        have = {c["attributes"]["capabilityType"] for c in
                api("GET", f"/v1/bundleIds/{bid}/bundleIdCapabilities")["data"]}
        for capability in (CAPABILITY[e] for e in entitlements):
            if capability not in have:
                api("POST", "/v1/bundleIdCapabilities", {"data": {
                    "type": "bundleIdCapabilities",
                    "attributes": {"capabilityType": capability},
                    "relationships": {"bundleId": {"data": {"type": "bundleIds", "id": bid}}}}})
                print(f"enabled {capability} on {identifier}")
        out[target] = bid
    return out


def certificate():
    key_path, cert_path, id_path = SIGNING / "dev.key", SIGNING / "dev.cer", SIGNING / "dev.id"
    if key_path.exists() and cert_path.exists() and id_path.exists():
        cert_id = id_path.read_text().strip()
        live = {c["id"] for c in api("GET", "/v1/certificates?limit=200")["data"]}
        if cert_id in live:
            return cert_id
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    csr = x509.CertificateSigningRequestBuilder().subject_name(x509.Name(
        [x509.NameAttribute(NameOID.COMMON_NAME, "paw development")])).sign(key, hashes.SHA256())
    created = api("POST", "/v1/certificates", {"data": {"type": "certificates", "attributes": {
        "certificateType": "DEVELOPMENT",
        "csrContent": csr.public_bytes(serialization.Encoding.PEM).decode()}}})
    key_path.write_bytes(key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL,
        serialization.NoEncryption()))
    key_path.chmod(0o600)
    cert_path.write_bytes(base64.b64decode(created["data"]["attributes"]["certificateContent"]))
    id_path.write_text(created["data"]["id"])
    print("created development certificate")
    return created["data"]["id"]


def profiles(bundles, cert_id, device_ids):
    existing = api("GET", "/v1/profiles?limit=200")["data"]
    missing_groups = []
    for target, bid in bundles.items():
        name = f"paw {target} development"
        for p in existing:
            if p["attributes"]["name"] == name:
                api("DELETE", f"/v1/profiles/{p['id']}")
        created = api("POST", "/v1/profiles", {"data": {"type": "profiles",
            "attributes": {"name": name, "profileType": "IOS_APP_DEVELOPMENT"},
            "relationships": {
                "bundleId": {"data": {"type": "bundleIds", "id": bid}},
                "certificates": {"data": [{"type": "certificates", "id": cert_id}]},
                "devices": {"data": [{"type": "devices", "id": d} for d in device_ids]}}}})
        attrs = created["data"]["attributes"]
        content = base64.b64decode(attrs["profileContent"])
        (SIGNING / f"{target}.mobileprovision").write_bytes(content)
        print(f"profile for {target}: {attrs['uuid']}, expires {attrs['expirationDate'][:10]}")
        # The App Store Connect API can switch App Groups on for an app ID but
        # cannot create a group or assign one to it.
        start, end = content.index(b"<?xml"), content.index(b"</plist>") + len(b"</plist>")
        granted = plistlib.loads(content[start:end])["Entitlements"].get(GROUPS, [])
        identifier, entitlements = TARGETS[target]
        for group in entitlements.get(GROUPS, []):
            if group not in granted:
                missing_groups.append((group, identifier))
    if missing_groups:
        print("\nThe profiles lack App Groups. At developer.apple.com, Certificates, Identifiers"
              " & Profiles:\n  Identifiers, App Groups: register each group below if it is not there;"
              "\n  Identifiers, each app ID below: App Groups, Configure, tick the group.")
        for group, identifier in missing_groups:
            print(f"  {group} on {identifier}")
        sys.exit("Then run this script again, to make the profiles anew.")


if __name__ == "__main__":
    if not DEVICES:
        sys.exit("Set IPHONE_UDID or IPAD_UDID in .env first.")
    device_ids = devices()
    profiles(bundle_ids(), certificate(), device_ids)
    print(f"signing files are in {SIGNING}")
