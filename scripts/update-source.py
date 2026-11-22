#!/usr/bin/env python3
"""scripts/update-source.py -- keep apps.json (this repository's SideStore / AltStore source) in step with the GitHub releases.

Run it with the isolated interpreter:  python3 -I scripts/update-source.py ...   (standard library only; Python 3.9 or newer)

  add or replace a version (the maintainer, after `gh release create`, or .github/workflows/release.yml):

    # from the IPA on disk: size, sha256, version, build, minimum iOS and the permissions are all read from the file
    python3 -I scripts/update-source.py --tag v0.1.0 --ipa dist/GameApp-rust-<build>.ipa

    # from the release's assets, without downloading the IPA (what the workflow does): size and sha256 come from the release,
    # version / build / minimum iOS from the IPA's Info.plist, read with HTTP range requests (a few hundred KB, not 40 MB)
    python3 -I scripts/update-source.py --tag v0.1.0 --ipa GameApp-rust-<build>.ipa --sha256 <hex> --size <bytes> \\
        --from-url https://github.com/Absenti/iBuilder/releases/download/v0.1.0/GameApp-rust-<build>.ipa --date 2026-11-20T12:00:00Z

    # nothing read from anywhere: every value is explicit
    python3 -I scripts/update-source.py --tag v0.1.0 --ipa GameApp-rust-<build>.ipa --sha256 <hex> --size <bytes> \\
        --version 1 --build 1 --min-os 26.0 --date 2026-11-20T12:00:00Z

  check the file:      python3 -I scripts/update-source.py --validate [--require-versions]
  test this script:    python3 -I scripts/update-source.py --self-test          (CI runs it; it builds a fake IPA, a fake Mach-O and a local range server)

What it does to apps.json: inserts the version in the app's `versions` array (v2 source format: version, buildVersion, date, localizedDescription,
downloadURL, size, sha256, minOSVersion), or replaces the entry with the same version and buildVersion; sorts the array newest first by `date`;
keeps the newest 10 (--keep); validates the whole file against a minimal schema BEFORE writing; writes atomically; changes nothing else (no key of the
file is reordered, nothing outside `versions` is touched, and running the same command twice gives the same bytes). Exit status: 0 done / valid, 1 error, 2 usage.

What is checked against the IPA when its bytes are read (the file on disk): CFBundleIdentifier equals the app's bundleIdentifier; `appPermissions.entitlements` is the
set of entitlements in the executable's signature (application-identifier and the team identifier excluded) and `appPermissions.privacy` the Info.plist's *UsageDescription
keys, because AltStore refuses to install an app whose permissions do not match its IPA (--update-permissions rewrites them from the IPA instead of failing). With --from-url
only Info.plist is read: the privacy keys are checked, the entitlements are not (said on stderr).

An app with an EMPTY `versions` array is valid for this script (the file before the first release) but NOT for the clients: SideStore (StoreApp.setVersions throws
MergeError.noVersions) and AltStore refuse to load a source that has an app without a version. `--validate --require-versions` fails on it.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import http.server
import io
import json
import os
import plistlib
import re
import stat
import struct
import sys
import tempfile
import threading
import urllib.parse
import urllib.request
import zipfile

REPO_DEFAULT = "Absenti/iBuilder"
KEEP_DEFAULT = 10
SOURCE_DEFAULT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "apps.json")
USER_AGENT = "ibuilder-update-source/1"

HEX64 = re.compile(r"^[0-9a-f]{64}$")
TAG_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
IPA_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,199}\.ipa$")
REPO_RE = re.compile(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$")
IOS_RE = re.compile(r"^[0-9]+(\.[0-9]+){0,2}$")
COLOR_RE = re.compile(r"^#[0-9A-Fa-f]{6}$")
CATEGORIES = {"developer", "entertainment", "games", "lifestyle", "other", "photo-video", "social", "utilities"}
IGNORED_ENTITLEMENTS = {"application-identifier", "com.apple.developer.team-identifier"}   # AltStore: need not be listed
VERSION_KEYS = ("version", "buildVersion", "date", "localizedDescription", "downloadURL", "size", "sha256", "minOSVersion")
NOTES_MAX = 2000


class Fail(Exception):
    pass


# ------------------------------------------------------------------------------------------------------------------------------------------------------------ dates
def parse_iso(text: str) -> dt.datetime:
    """ISO 8601 with a Z or an offset (or none: UTC) -> aware UTC datetime. Raises Fail."""
    if not isinstance(text, str):
        raise Fail("a date must be a string")
    s = text.strip()
    if s.endswith(("Z", "z")):
        s = s[:-1] + "+00:00"
    try:
        d = dt.datetime.fromisoformat(s)
    except ValueError:
        raise Fail("not an ISO 8601 date: %r" % text)
    if d.tzinfo is None:
        d = d.replace(tzinfo=dt.timezone.utc)
    return d.astimezone(dt.timezone.utc)


def iso(d: dt.datetime) -> str:
    return d.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# ------------------------------------------------------------------------------------------------------------------------------------------------------ the schema
def check_source(doc, require_versions=False):
    """Minimal schema of the v2 source format as SideStore (StoreApp.swift, AppVersion.swift) and AltStore read it. Returns (errors, warnings)."""
    errors, warnings = [], []

    def err(where, msg):
        errors.append("%s: %s" % (where, msg))

    def is_str(v):
        return isinstance(v, str) and v.strip() != ""

    def is_url(v):
        return isinstance(v, str) and re.match(r"^https://[^\s/]+(/\S*)?$", v) is not None

    if not isinstance(doc, dict):
        return ["the file must hold a JSON object"], warnings
    if not is_str(doc.get("name")):
        err("name", "required, a non-empty string")
    for k in ("identifier", "subtitle", "description"):
        if k in doc and not is_str(doc[k]):
            err(k, "must be a non-empty string when present")
    for k in ("iconURL", "headerURL", "website"):
        if k in doc and not is_url(doc[k]):
            err(k, "must be an https URL when present")
    if "tintColor" in doc and not (isinstance(doc["tintColor"], str) and COLOR_RE.match(doc["tintColor"])):
        err("tintColor", "must look like #RRGGBB")
    news = doc.get("news", [])
    if not isinstance(news, list):
        err("news", "must be an array")
    else:
        for i, n in enumerate(news):
            w = "news[%d]" % i
            if not isinstance(n, dict):
                err(w, "must be an object")
                continue
            for k in ("title", "identifier", "caption"):
                if not is_str(n.get(k)):
                    err(w + "." + k, "required, a non-empty string")
            try:
                parse_iso(n.get("date"))
            except Fail as e:
                err(w + ".date", str(e))
    apps = doc.get("apps")
    if not isinstance(apps, list):
        err("apps", "required, an array")
        return errors, warnings
    if not apps:
        warnings.append("apps is empty: the source lists nothing")
    seen_ids = set()
    for ai, app in enumerate(apps):
        w = "apps[%d]" % ai
        if not isinstance(app, dict):
            err(w, "must be an object")
            continue
        for k in ("name", "bundleIdentifier", "developerName", "localizedDescription"):
            if not is_str(app.get(k)):
                err(w + "." + k, "required, a non-empty string")
        if not is_url(app.get("iconURL")):
            err(w + ".iconURL", "required, an https URL")
        bid = app.get("bundleIdentifier")
        if isinstance(bid, str):
            if bid in seen_ids:
                err(w + ".bundleIdentifier", "duplicate %r" % bid)
            seen_ids.add(bid)
        if "subtitle" in app and not is_str(app["subtitle"]):
            err(w + ".subtitle", "must be a non-empty string when present")
        if "tintColor" in app and not (isinstance(app["tintColor"], str) and COLOR_RE.match(app["tintColor"])):
            err(w + ".tintColor", "must look like #RRGGBB")
        if "category" in app and app["category"] not in CATEGORIES:
            err(w + ".category", "must be one of %s" % ", ".join(sorted(CATEGORIES)))
        perms = app.get("appPermissions")
        if perms is None:
            err(w + ".appPermissions", "required (AltStore refuses an app whose permissions are not listed): {entitlements: [...], privacy: {...}}")
        elif not isinstance(perms, dict):
            err(w + ".appPermissions", "must be an object")
        else:
            ents = perms.get("entitlements")
            if not (isinstance(ents, list) and all(is_str(e) for e in ents)):
                err(w + ".appPermissions.entitlements", "required, an array of strings")
            priv = perms.get("privacy")
            if not (isinstance(priv, dict) and all(is_str(k) and isinstance(v, str) for k, v in priv.items())):
                err(w + ".appPermissions.privacy", "required, an object of <UsageDescription key>: <text>")
        versions = app.get("versions")
        if not isinstance(versions, list):
            err(w + ".versions", "required, an array (newest first)")
            continue
        if not versions:
            msg = "%s has no version: SideStore and AltStore refuse a source with an app that has none (add the first release with scripts/update-source.py)" % w
            (errors if require_versions else warnings).append(msg)
        seen, last = set(), None
        for vi, v in enumerate(versions):
            vw = "%s.versions[%d]" % (w, vi)
            if not isinstance(v, dict):
                err(vw, "must be an object")
                continue
            for k in ("version", "buildVersion"):
                if not is_str(v.get(k)):
                    err(vw + "." + k, "required, a non-empty string")
            if not is_url(v.get("downloadURL")):
                err(vw + ".downloadURL", "required, an https URL")
            size = v.get("size")
            if not (isinstance(size, int) and not isinstance(size, bool) and size > 0):
                err(vw + ".size", "required, a positive integer (bytes)")
            if not (isinstance(v.get("sha256"), str) and HEX64.match(v["sha256"])):
                err(vw + ".sha256", "required, 64 lowercase hex digits")
            if not (isinstance(v.get("minOSVersion"), str) and IOS_RE.match(v["minOSVersion"])):
                err(vw + ".minOSVersion", "required, like 26.0")
            if "localizedDescription" in v and not isinstance(v["localizedDescription"], str):
                err(vw + ".localizedDescription", "must be a string when present")
            try:
                d = parse_iso(v.get("date"))
                if last is not None and d > last:
                    err(vw + ".date", "versions must be newest first (this one is newer than the one before it)")
                last = d
            except Fail as e:
                err(vw + ".date", str(e))
            key = (v.get("version"), v.get("buildVersion"))
            if key in seen:
                err(vw, "duplicate version/buildVersion %r (AltStore wants each entry to differ in one of them)" % (key,))
            seen.add(key)
    return errors, warnings


# --------------------------------------------------------------------------------------------------------------------------------------------------- the JSON file
def load_source(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except OSError as e:
        raise Fail("cannot read %s: %s" % (path, e))
    except ValueError as e:
        raise Fail("%s is not valid JSON: %s" % (path, e))


def dump_source(doc) -> str:
    return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"


def save_source(path, text):
    d = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(prefix=".apps-json-", dir=d)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        try:   # mkstemp makes the file 0600: keep the mode of the file it replaces (a new file gets the usual 0644 & ~umask)
            mode = stat.S_IMODE(os.stat(path).st_mode)
        except OSError:
            um = os.umask(0); os.umask(um); mode = 0o666 & ~um
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


# ------------------------------------------------------------------------------------------------------------------------------------------- reading an IPA's facts
def find_app_dir(names):
    apps = sorted({n.split("/")[1] for n in names if re.match(r"^Payload/[^/]+\.app/", n)})
    if len(apps) != 1:
        raise Fail("the IPA must hold exactly one Payload/<name>.app (found %d)" % len(apps))
    return "Payload/" + apps[0]


def info_plist_facts(info):
    def need(k):
        v = info.get(k)
        if not isinstance(v, str) or not v:
            raise Fail("Info.plist has no %s" % k)
        return v
    privacy = {k: v for k, v in info.items() if k.endswith("UsageDescription") and isinstance(v, str)}
    return {
        "bundle_id": need("CFBundleIdentifier"),
        "version": need("CFBundleShortVersionString"),
        "build": need("CFBundleVersion"),
        "min_os": need("MinimumOSVersion"),
        "executable": info.get("CFBundleExecutable"),
        "privacy": dict(sorted(privacy.items())),
    }


def macho_entitlements(data: bytes):
    """Entitlements (dict) from the embedded code signature of a Mach-O (thin or fat, arm64 preferred), or None when there is none."""
    magic_be = struct.unpack_from(">I", data, 0)[0]
    base = 0
    if magic_be in (0xCAFEBABE, 0xCAFEBABF):
        wide = magic_be == 0xCAFEBABF
        n = struct.unpack_from(">I", data, 4)[0]
        fmt, size = (">iiQQII", 32) if wide else (">iiIIi", 20)
        picks = []
        for i in range(n):
            ct, _st, off, _sz = struct.unpack_from(fmt, data, 8 + i * size)[:4]
            picks.append((ct, off))
        if not picks:
            return None
        base = next((off for ct, off in picks if ct == 0x0100000C), picks[0][1])
    if struct.unpack_from("<I", data, base)[0] != 0xFEEDFACF:
        raise Fail("the executable is not a 64-bit little-endian Mach-O")
    ncmds = struct.unpack_from("<I", data, base + 16)[0]
    off = base + 32
    for _ in range(ncmds):
        cmd, size = struct.unpack_from("<II", data, off)
        if cmd == 0x1D:   # LC_CODE_SIGNATURE
            dataoff, datasize = struct.unpack_from("<II", data, off + 8)
            sig = data[base + dataoff: base + dataoff + datasize]
            if len(sig) < 12 or struct.unpack_from(">I", sig, 0)[0] != 0xFADE0CC0:
                return None
            count = struct.unpack_from(">I", sig, 8)[0]
            for i in range(count):
                _type, boff = struct.unpack_from(">II", sig, 12 + i * 8)
                if boff + 8 <= len(sig) and struct.unpack_from(">I", sig, boff)[0] == 0xFADE7171:   # XML entitlements blob
                    blen = struct.unpack_from(">I", sig, boff + 4)[0]
                    return plistlib.loads(sig[boff + 8: boff + blen])
            return None
        off += size
    return None


def ipa_facts(zf: zipfile.ZipFile, with_entitlements: bool):
    names = zf.namelist()
    app = find_app_dir(names)
    try:
        info = plistlib.loads(zf.read(app + "/Info.plist"))
    except KeyError:
        raise Fail("the IPA has no %s/Info.plist" % app)
    facts = info_plist_facts(info)
    facts["entitlements"] = None
    if with_entitlements:
        exe = facts["executable"]
        if not exe:
            raise Fail("Info.plist has no CFBundleExecutable")
        try:
            ents = macho_entitlements(zf.read("%s/%s" % (app, exe)))
        except KeyError:
            raise Fail("the IPA has no executable %s/%s" % (app, exe))
        facts["entitlements"] = sorted(k for k in (ents or {}) if k not in IGNORED_ENTITLEMENTS)
    return facts


def sha256_and_size(path):
    h, n = hashlib.sha256(), 0
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
            n += len(chunk)
    return h.hexdigest(), n


# ------------------------------------------------------------------------------------------------------------------------------- reading a remote zip by range
class RangeUnsupported(Exception):
    pass


class HttpRangeFile(io.RawIOBase):
    """A read-only, seekable view of a remote file through HTTP range requests (zipfile only needs seek/read: the central directory is at the end of the archive)."""
    BLOCK = 256 * 1024

    def __init__(self, url):
        super().__init__()
        self.url = url
        self.pos = 0
        self.requests = 0
        self.bytes_read = 0
        self._cache = {}
        final, total, data = self._fetch(0, 0)
        self.url = final            # the redirect target (a signed CDN URL for a GitHub release asset): asked directly from now on
        self.size = total

    def _fetch(self, start, end):
        req = urllib.request.Request(self.url, headers={"Range": "bytes=%d-%d" % (start, end), "User-Agent": USER_AGENT, "Accept-Encoding": "identity"})
        with urllib.request.urlopen(req, timeout=60) as r:
            if r.status != 206:
                raise RangeUnsupported("HTTP %s instead of 206 for a range request" % r.status)
            m = re.match(r"^bytes (\d+)-(\d+)/(\d+)$", r.headers.get("Content-Range", ""))
            if not m:
                raise RangeUnsupported("no usable Content-Range header")
            data = r.read()
            self.requests += 1
            self.bytes_read += len(data)
            return r.geturl(), int(m.group(3)), data

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, offset, whence=io.SEEK_SET):
        if whence == io.SEEK_SET:
            self.pos = offset
        elif whence == io.SEEK_CUR:
            self.pos += offset
        else:
            self.pos = self.size + offset
        self.pos = max(0, self.pos)
        return self.pos

    def _block(self, index):
        if index not in self._cache:
            if len(self._cache) >= 8:
                self._cache.pop(next(iter(self._cache)))
            start = index * self.BLOCK
            end = min(start + self.BLOCK, self.size) - 1
            self._cache[index] = self._fetch(start, end)[2]
        return self._cache[index]

    def readinto(self, b):
        want = min(len(b), max(0, self.size - self.pos))
        out = bytearray()
        while len(out) < want:
            index, inner = divmod(self.pos + len(out), self.BLOCK)
            out += self._block(index)[inner: inner + (want - len(out))]
        b[:len(out)] = out
        self.pos += len(out)
        return len(out)


def download_to_temp(url, expect_sha, expect_size):
    """Fallback when ranges do not work: the whole file, checked against the sha256 and the size the release states."""
    fd, tmp = tempfile.mkstemp(suffix=".ipa")
    h, n = hashlib.sha256(), 0
    try:
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=120) as r, os.fdopen(fd, "wb") as f:
            for chunk in iter(lambda: r.read(1 << 20), b""):
                f.write(chunk)
                h.update(chunk)
                n += len(chunk)
    except BaseException:
        os.unlink(tmp)
        raise
    if n != expect_size or h.hexdigest() != expect_sha:
        os.unlink(tmp)
        raise Fail("the downloaded IPA is %d bytes, sha256 %s; the release says %d bytes, sha256 %s" % (n, h.hexdigest(), expect_size, expect_sha))
    return tmp


# ----------------------------------------------------------------------------------------------------------------------------------------------- updating apps.json
def pick_app(doc, bundle_id):
    apps = doc.get("apps", [])
    if bundle_id:
        for a in apps:
            if a.get("bundleIdentifier") == bundle_id:
                return a
        raise Fail("apps.json has no app with bundleIdentifier %s" % bundle_id)
    if len(apps) != 1:
        raise Fail("apps.json lists %d apps: say which with --bundle-id" % len(apps))
    return apps[0]


def put_version(app, entry, keep):
    """Replace the entry with the same (version, buildVersion) or add the new one in front, sort newest first (stable: the new entry wins a tie), keep `keep`."""
    key = (entry["version"], entry["buildVersion"])
    old = [v for v in app["versions"] if (v.get("version"), v.get("buildVersion")) != key]
    merged = [entry] + old
    merged.sort(key=lambda v: parse_iso(v["date"]), reverse=True)     # sorted() is stable
    app["versions"] = merged[:keep]


def run_update(a):
    if not TAG_RE.match(a.tag):
        raise Fail("--tag %r: only letters, digits, . _ - are accepted" % a.tag)
    if not REPO_RE.match(a.repo):
        raise Fail("--repo must look like owner/name")
    if a.keep < 1:
        raise Fail("--keep must be at least 1")
    doc = load_source(a.file)
    errors, _ = check_source(doc)
    if errors:
        raise Fail("%s is not valid before the update:\n  %s" % (a.file, "\n  ".join(errors)))
    app = pick_app(doc, a.bundle_id)

    local = os.path.isfile(a.ipa)
    name = os.path.basename(a.ipa)
    if not local and ("/" in a.ipa or os.sep in a.ipa):
        raise Fail("--ipa %r is neither a file nor a bare asset name" % a.ipa)
    if not IPA_NAME_RE.match(name):
        raise Fail("the IPA's file name %r is not acceptable (letters, digits, . _ + - and a .ipa suffix)" % name)

    sha, size = (a.sha256.lower() if a.sha256 else None), a.size
    facts, notes = None, []
    tmp_ipa = None
    try:
        if local:
            real_sha, real_size = sha256_and_size(a.ipa)
            for what, given, real in (("--sha256", sha, real_sha), ("--size", size, real_size)):
                if given is not None and given != real:
                    raise Fail("%s %s does not match the file %s (%s)" % (what, given, a.ipa, real))
            sha, size = real_sha, real_size
            with zipfile.ZipFile(a.ipa) as zf:
                facts = ipa_facts(zf, with_entitlements=True)
        elif a.from_url:
            if sha is None or size is None:
                raise Fail("--from-url needs --sha256 and --size (from the release's assets)")
            if a.download:
                tmp_ipa = download_to_temp(a.from_url, sha, size)
                with zipfile.ZipFile(tmp_ipa) as zf:
                    facts = ipa_facts(zf, with_entitlements=True)
            else:
                try:
                    rf = HttpRangeFile(a.from_url)
                    if rf.size != size:
                        raise Fail("the file at --from-url is %d bytes, --size says %d" % (rf.size, size))
                    with zipfile.ZipFile(rf) as zf:
                        facts = ipa_facts(zf, with_entitlements=False)
                    print("update-source: read Info.plist with %d range request(s), %d bytes of %d" % (rf.requests, rf.bytes_read, rf.size), file=sys.stderr)
                    notes.append("the entitlements were NOT checked against the IPA (ranged read); run with the IPA on disk, or --download, to check them")
                except (RangeUnsupported, OSError, zipfile.BadZipFile) as e:
                    print("update-source: ranged read failed (%s); downloading the whole file instead" % e, file=sys.stderr)
                    tmp_ipa = download_to_temp(a.from_url, sha, size)
                    with zipfile.ZipFile(tmp_ipa) as zf:
                        facts = ipa_facts(zf, with_entitlements=True)
    finally:
        if tmp_ipa:
            os.unlink(tmp_ipa)

    if sha is None or size is None:
        raise Fail("give --sha256 and --size (or a local IPA, which they are computed from)")
    if not HEX64.match(sha):
        raise Fail("--sha256 must be 64 hex digits")
    if not isinstance(size, int) or size <= 0:
        raise Fail("--size must be a positive number of bytes")

    values = {"version": a.version, "build": a.build, "min_os": a.min_os}
    if facts:
        if facts["bundle_id"] != app["bundleIdentifier"]:
            raise Fail("the IPA's CFBundleIdentifier is %s, the app in apps.json is %s" % (facts["bundle_id"], app["bundleIdentifier"]))
        for k, given in list(values.items()):
            if given is not None and given != facts[k]:
                raise Fail("--%s %s contradicts the IPA (%s): the source must say what the IPA says" % (k.replace("_", "-"), given, facts[k]))
            values[k] = facts[k]
        perms = app["appPermissions"]
        if facts["privacy"] != perms["privacy"]:
            if a.update_permissions:
                perms["privacy"] = facts["privacy"]
            else:
                raise Fail("appPermissions.privacy differs from the IPA's Info.plist (AltStore would refuse the install)\n  apps.json: %s\n  IPA:       %s\n  (--update-permissions rewrites it)"
                           % (json.dumps(perms["privacy"], sort_keys=True), json.dumps(facts["privacy"], sort_keys=True)))
        if facts["entitlements"] is not None and sorted(perms["entitlements"]) != facts["entitlements"]:
            if a.update_permissions:
                perms["entitlements"] = facts["entitlements"]
            else:
                raise Fail("appPermissions.entitlements differs from the IPA's signature (AltStore would refuse the install)\n  apps.json: %s\n  IPA:       %s\n  (--update-permissions rewrites it)"
                           % (sorted(perms["entitlements"]), facts["entitlements"]))
    elif a.update_permissions:
        raise Fail("--update-permissions needs the IPA's bytes (a local file or --download)")
    for k, v in values.items():
        if not v:
            raise Fail("no value for --%s: pass it, or give a local IPA / --from-url to read it from" % k.replace("_", "-"))
    if not IOS_RE.match(values["min_os"]):
        raise Fail("minimum iOS %r is not like 26.0" % values["min_os"])

    date = parse_iso(a.date) if a.date else dt.datetime.now(dt.timezone.utc)
    text = a.notes
    if a.notes_file:
        with open(a.notes_file, "r", encoding="utf-8") as f:
            text = f.read()
    text = (text or "").strip()
    if len(text) > NOTES_MAX:
        text = text[:NOTES_MAX - 1].rstrip() + "…"

    entry = {"version": values["version"], "buildVersion": values["build"], "date": iso(date)}
    if text:
        entry["localizedDescription"] = text
    entry["downloadURL"] = "https://github.com/%s/releases/download/%s/%s" % (a.repo, urllib.parse.quote(a.tag, safe=""), urllib.parse.quote(name, safe=""))
    entry["size"] = size
    entry["sha256"] = sha
    entry["minOSVersion"] = values["min_os"]
    put_version(app, entry, a.keep)

    errors, _ = check_source(doc)
    if errors:
        raise Fail("the result would not be valid; nothing written:\n  %s" % "\n  ".join(errors))
    new_text = dump_source(doc)
    with open(a.file, "r", encoding="utf-8") as f:
        old_text = f.read()
    for n in notes:
        print("update-source: note: " + n, file=sys.stderr)
    if new_text == old_text:
        print("update-source: %s already lists %s (build %s) as given: nothing to change" % (os.path.basename(a.file), entry["version"], entry["buildVersion"]))
    elif a.dry_run:
        print("update-source: --dry-run: would write %s with %d version(s), newest %s (build %s) %s" % (a.file, len(app["versions"]), app["versions"][0]["version"], app["versions"][0]["buildVersion"], app["versions"][0]["date"]))
    else:
        save_source(a.file, new_text)
        print("update-source: %s now lists %d version(s) of %s, newest %s (build %s, %s, %d bytes)" % (a.file, len(app["versions"]), app["bundleIdentifier"], app["versions"][0]["version"], app["versions"][0]["buildVersion"], app["versions"][0]["date"], app["versions"][0]["size"]))
    return 0


def run_validate(a):
    doc = load_source(a.file)
    errors, warnings = check_source(doc, require_versions=a.require_versions)
    for w in warnings:
        print("update-source: warning: " + w, file=sys.stderr)
    if errors:
        print("update-source: %s is NOT valid:" % a.file, file=sys.stderr)
        for e in errors:
            print("  " + e, file=sys.stderr)
        return 1
    n = sum(len(x.get("versions", [])) for x in doc.get("apps", []))
    print("update-source: %s is valid (%d app(s), %d version(s))" % (a.file, len(doc.get("apps", [])), n))
    return 0


# ----------------------------------------------------------------------------------------------------------------------------------------------------- the self test
def _fake_macho(entitlements):
    xml = plistlib.dumps(entitlements)
    blob = struct.pack(">II", 0xFADE7171, 8 + len(xml)) + xml
    sig = struct.pack(">III", 0xFADE0CC0, 12 + 8 + len(blob), 1) + struct.pack(">II", 5, 12 + 8) + blob
    head = struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 16, 0, 0) + struct.pack("<IIII", 0x1D, 16, 64, len(sig))
    return head + b"\0" * (64 - len(head)) + sig


def _fake_ipa(path, version="1", build="1", min_os="26.0", ents=None, privacy=None, bundle_id="dev.example.app", padding=0):
    info = {"CFBundleIdentifier": bundle_id, "CFBundleShortVersionString": version, "CFBundleVersion": build, "MinimumOSVersion": min_os, "CFBundleExecutable": "App"}
    info.update(privacy or {})
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("Payload/App.app/Info.plist", plistlib.dumps(info))
        z.writestr("Payload/App.app/App", _fake_macho(ents or {}))
        if padding:   # incompressible bytes in the middle, so that a ranged read of the ends is visibly smaller than the file
            z.writestr(zipfile.ZipInfo("Payload/App.app/pad.bin"), os.urandom(padding), compress_type=zipfile.ZIP_STORED)
        for i in range(200):   # many entries: a real central directory
            z.writestr("Payload/App.app/res/f%03d.bin" % i, os.urandom(300))


class _RangeHandler(http.server.BaseHTTPRequestHandler):
    blob = b""

    def log_message(self, *args):
        pass

    def do_GET(self):
        data, rng = self.blob, self.headers.get("Range")
        m = re.match(r"^bytes=(\d+)-(\d+)$", rng or "")
        if not m:
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        a, b = int(m.group(1)), min(int(m.group(2)), len(data) - 1)
        self.send_response(206)
        self.send_header("Content-Range", "bytes %d-%d/%d" % (a, b, len(data)))
        self.send_header("Content-Length", str(b - a + 1))
        self.end_headers()
        self.wfile.write(data[a:b + 1])


def run_self_test():
    import argparse as ap
    failures = []

    def ok(cond, what):
        print("  %s %s" % ("PASS" if cond else "FAIL", what))
        if not cond:
            failures.append(what)

    def raises(fn, text):
        try:
            fn()
        except Fail as e:
            return text in str(e)
        return False

    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "apps.json")
        base = {"name": "Test", "identifier": "x.y", "apps": [{
            "name": "App", "bundleIdentifier": "dev.example.app", "developerName": "Me", "localizedDescription": "d",
            "iconURL": "https://example.com/i.png", "tintColor": "#112233",
            "appPermissions": {"entitlements": ["com.apple.developer.kernel.increased-memory-limit"], "privacy": {"NSPhotoLibraryAddUsageDescription": "why"}},
            "versions": []}], "news": []}
        open(src, "w").write(dump_source(base))

        def ns(**kw):
            d0 = dict(file=src, tag="v1", ipa="", sha256=None, size=None, version=None, build=None, min_os=None, date=None, notes=None, notes_file=None, from_url=None,
                      download=False, repo=REPO_DEFAULT, bundle_id=None, keep=KEEP_DEFAULT, update_permissions=False, dry_run=False)
            d0.update(kw)
            return ap.Namespace(**d0)

        ents = {"com.apple.developer.kernel.increased-memory-limit": True, "application-identifier": "T.dev.example.app"}
        priv = {"NSPhotoLibraryAddUsageDescription": "why"}
        ipa1 = os.path.join(d, "App-1.ipa")
        _fake_ipa(ipa1, "1", "1", ents=ents, privacy=priv, padding=3000000)
        print("self-test: schema")
        errs, warns = check_source(base)
        ok(not errs and any("no version" in w for w in warns), "the initial file (empty versions) is valid, with a warning")
        ok(any("no version" in e for e in check_source(base, require_versions=True)[0]), "--require-versions turns the warning into an error")
        bad = json.loads(json.dumps(base))
        bad["apps"][0]["tintColor"] = "red"
        del bad["apps"][0]["appPermissions"]
        ok(len(check_source(bad)[0]) == 2, "a bad tintColor and missing appPermissions are two errors")

        print("self-test: add / replace / limit (from a local IPA)")
        run_update(ns(ipa=ipa1, tag="v1", date="2026-01-01T00:00:00Z", notes="first"))
        doc = load_source(src)
        v = doc["apps"][0]["versions"]
        ok(len(v) == 1 and v[0]["version"] == "1" and v[0]["buildVersion"] == "1" and v[0]["minOSVersion"] == "26.0", "first version added from the IPA's own Info.plist")
        ok(v[0]["sha256"] == sha256_and_size(ipa1)[0] and v[0]["size"] == os.path.getsize(ipa1), "sha256 and size are those of the file")
        ok(v[0]["downloadURL"] == "https://github.com/Absenti/iBuilder/releases/download/v1/App-1.ipa", "downloadURL is the release asset URL")
        ok(list(v[0]) == list(VERSION_KEYS), "the entry's keys are in the documented order")
        before = open(src).read()
        run_update(ns(ipa=ipa1, tag="v1", date="2026-01-01T00:00:00Z", notes="first"))
        ok(open(src).read() == before, "the same command twice leaves the file byte-identical")
        ok(list(doc) == ["name", "identifier", "apps", "news"] and list(doc["apps"][0])[:3] == ["name", "bundleIdentifier", "developerName"], "no other key was reordered")
        run_update(ns(ipa=ipa1, tag="v1b", date="2026-01-02T00:00:00Z"))
        v = load_source(src)["apps"][0]["versions"]
        ok(len(v) == 1 and v[0]["date"] == "2026-01-02T00:00:00Z" and v[0]["downloadURL"].endswith("/v1b/App-1.ipa"), "the same version and build REPLACES the entry")
        for i in range(2, 14):
            p = os.path.join(d, "App-%d.ipa" % i)
            _fake_ipa(p, str(i), str(i), ents=ents, privacy=priv)
            run_update(ns(ipa=p, tag="v%d" % i, date="2026-02-%02dT00:00:00Z" % i))
        v = load_source(src)["apps"][0]["versions"]
        ok(len(v) == 10 and v[0]["version"] == "13" and v[-1]["version"] == "4", "keeps the newest 10, newest first")
        old = os.path.join(d, "App-0.ipa")
        _fake_ipa(old, "0", "0", ents=ents, privacy=priv)
        run_update(ns(ipa=old, tag="v0", date="2026-01-01T00:00:00Z", keep=20))
        v = load_source(src)["apps"][0]["versions"]
        ok(v[0]["version"] == "13" and v[-1]["version"] == "0", "an older release added later is sorted behind the newer ones, not put first")
        ok(check_source(load_source(src))[0] == [], "the file is valid after all that")

        print("self-test: refusals (nothing is written)")
        before = open(src).read()
        other = os.path.join(d, "Other.ipa")
        _fake_ipa(other, "9", "9", ents=ents, privacy=priv, bundle_id="dev.other.app")
        ok(raises(lambda: run_update(ns(ipa=other)), "CFBundleIdentifier"), "a different bundle id")
        wrong = os.path.join(d, "Wrong.ipa")
        _fake_ipa(wrong, "9", "9", ents={"com.apple.developer.kernel.extended-virtual-addressing": True}, privacy=priv)
        ok(raises(lambda: run_update(ns(ipa=wrong)), "appPermissions.entitlements"), "entitlements that differ from the signature")
        _fake_ipa(wrong, "9", "9", ents=ents, privacy={"NSCameraUsageDescription": "x"})
        ok(raises(lambda: run_update(ns(ipa=wrong)), "appPermissions.privacy"), "privacy keys that differ from Info.plist")
        ok(raises(lambda: run_update(ns(ipa=ipa1, version="7")), "contradicts"), "--version that contradicts the IPA")
        ok(raises(lambda: run_update(ns(ipa=ipa1, sha256="0" * 64)), "does not match"), "--sha256 that does not match the file")
        ok(raises(lambda: run_update(ns(ipa="X.ipa", sha256="a" * 64, size=5)), "no value for --version"), "an asset name with nothing to read the version from")
        ok(raises(lambda: run_update(ns(ipa="../X.ipa", sha256="a" * 64, size=5, version="1", build="1", min_os="26.0")), "neither a file nor"), "a path-like asset name")
        ok(raises(lambda: run_update(ns(ipa=ipa1, tag="v1/../x")), "--tag"), "a tag with a slash")
        ok(open(src).read() == before, "apps.json was not touched by any refusal")
        run_update(ns(ipa=wrong, update_permissions=True, tag="vperm", date="2027-01-01T00:00:00Z"))
        ok(load_source(src)["apps"][0]["appPermissions"]["privacy"] == {"NSCameraUsageDescription": "x"}, "--update-permissions rewrites appPermissions from the IPA")

        print("self-test: explicit values, and the ranged read over a local HTTP server")
        open(src, "w").write(dump_source(base))
        sha, size = sha256_and_size(ipa1)
        run_update(ns(ipa="App-1.ipa", sha256=sha, size=size, version="1", build="1", min_os="26.0", date="2026-03-01T01:02:03+01:00"))
        v = load_source(src)["apps"][0]["versions"]
        ok(len(v) == 1 and v[0]["date"] == "2026-03-01T00:02:03Z", "explicit values; a +01:00 date is written as UTC")
        open(src, "w").write(dump_source(base))
        _RangeHandler.blob = open(ipa1, "rb").read()
        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _RangeHandler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            url = "http://127.0.0.1:%d/App-1.ipa" % srv.server_address[1]
            rf = HttpRangeFile(url)
            with zipfile.ZipFile(rf) as zf:
                f = ipa_facts(zf, with_entitlements=False)
            ok(f["version"] == "1" and f["build"] == "1" and f["min_os"] == "26.0" and f["privacy"] == priv, "Info.plist read through range requests")
            ok(rf.bytes_read < size and rf.requests >= 2, "fewer bytes than the file (%d of %d, %d requests)" % (rf.bytes_read, size, rf.requests))
            run_update(ns(ipa="App-1.ipa", sha256=sha, size=size, from_url=url, date="2026-03-01T00:00:00Z"))
            ok(load_source(src)["apps"][0]["versions"][0]["sha256"] == sha, "--from-url adds the version")
            open(src, "w").write(dump_source(base))
            run_update(ns(ipa="App-1.ipa", sha256=sha, size=size, from_url=url, download=True, date="2026-03-01T00:00:00Z"))
            ok(load_source(src)["apps"][0]["versions"][0]["sha256"] == sha, "--download: whole file, hash and size checked, entitlements checked too")
            ok(raises(lambda: run_update(ns(ipa="App-1.ipa", sha256="b" * 64, size=size, from_url=url, download=True)), "the release says"), "--download refuses a file whose sha256 is not the release's")
            ok(raises(lambda: run_update(ns(ipa="App-1.ipa", sha256=sha, size=size + 1, from_url=url)), "--size says"), "a size that is not the file's")
        finally:
            srv.shutdown()
        e = macho_entitlements(_fake_macho({"a": True}))
        ok(e == {"a": True}, "entitlements parsed from a Mach-O code signature")

    if failures:
        print("self-test: %d FAILED" % len(failures), file=sys.stderr)
        return 1
    print("self-test: all passed")
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(description="Keep apps.json (the SideStore / AltStore source) in step with the releases. See the header of this file.")
    p.add_argument("--file", default=SOURCE_DEFAULT, help="the source file (default: apps.json at the repository root)")
    p.add_argument("--validate", action="store_true", help="only check the file against the minimal schema")
    p.add_argument("--require-versions", action="store_true", help="with --validate: an app without a version is an error (what the clients do)")
    p.add_argument("--self-test", action="store_true", help="run this script's own tests (no network, no files outside a temporary folder)")
    p.add_argument("--tag", help="the release tag, e.g. v0.1.0")
    p.add_argument("--ipa", help="the IPA: a file on disk, or the name of the release asset")
    p.add_argument("--sha256", help="sha256 of the IPA (from the release's .sha256 asset); computed when the IPA is a file")
    p.add_argument("--size", type=int, help="size of the IPA in bytes (from the release); computed when the IPA is a file")
    p.add_argument("--version", help="CFBundleShortVersionString (read from the IPA when it is given)")
    p.add_argument("--build", help="CFBundleVersion (read from the IPA when it is given)")
    p.add_argument("--min-os", dest="min_os", help="MinimumOSVersion (read from the IPA when it is given)")
    p.add_argument("--date", help="release date, ISO 8601 (default: now, UTC); the workflow passes the release's published_at")
    p.add_argument("--notes", help="what's new, plain text (shown by the clients; cut at %d characters)" % NOTES_MAX)
    p.add_argument("--notes-file", help="read --notes from this file")
    p.add_argument("--from-url", help="read version, build and minimum iOS from the IPA at this URL by range requests (the whole file only if ranges fail)")
    p.add_argument("--download", action="store_true", help="with --from-url: download the whole IPA (checked against --sha256 and --size) and check the entitlements too")
    p.add_argument("--repo", default=REPO_DEFAULT, help="owner/name of the GitHub repository whose release holds the IPA (default %(default)s)")
    p.add_argument("--bundle-id", help="which app of the file (needed only when it lists several)")
    p.add_argument("--keep", type=int, default=KEEP_DEFAULT, help="how many versions to keep (default %(default)s)")
    p.add_argument("--update-permissions", action="store_true", help="rewrite appPermissions from the IPA instead of refusing when they differ")
    p.add_argument("--dry-run", action="store_true", help="do everything but write the file")
    a = p.parse_args(argv)
    try:
        if a.self_test:
            return run_self_test()
        if a.validate:
            return run_validate(a)
        if not a.tag or not a.ipa:
            p.error("--tag and --ipa are required (or use --validate / --self-test)")
        return run_update(a)
    except Fail as e:
        print("update-source: " + str(e), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
