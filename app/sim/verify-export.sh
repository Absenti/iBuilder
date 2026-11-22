#!/bin/bash
# app/sim/verify-export.sh <exported.ipa> [full|slim] [expected bundle id prefix] -- the structure of a game .ipa the app exported (GameAppExport.cs), read back from the archive.
# Prints the layout (what is where, sizes, the first entries of each folder), the Info.plist identity, the framework's Info.plist, the manifest, and PASS/FAIL checks:
# one .app under Payload/, a bundle id of its own, Frameworks/DmGameProbe.framework (Mach-O + its Info.plist), ExportedGame/{manifest.tsv,extract/flat,shaders/<id>,documents/...},
# RomFS.img inside only for a full export, NO Python, no embedded.mobileprovision, no _CodeSignature, no ExportedGame inside ExportedGame, no identity in the executable's signature.
set -uo pipefail
IPA="${1:?usage: verify-export.sh <exported.ipa> [full|slim] [bundle id prefix]}"; KIND="${2:-full}"; PREFIX="${3:-dev.ibuilder.app}"
say() { printf '\033[1m%s\033[0m\n' "$*"; }; pass() { printf '  \033[32mPASS\033[0m %s\n' "$*"; }; fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; BAD=1; }
BAD=0
[ -f "$IPA" ] || { echo "no such file: $IPA" >&2; exit 1; }
say "archive: $IPA  $(stat -f %z "$IPA") bytes"
python3 -I - "$IPA" <<'PY'
import sys, zipfile, collections
z = zipfile.ZipFile(sys.argv[1]); infos = [i for i in z.infolist() if not i.is_dir()]
apps = sorted({i.filename.split('/')[1] for i in infos if i.filename.startswith('Payload/')})
print('  %d entries, %.1f MB uncompressed, %.1f MB stored; apps under Payload/: %s' % (len(infos), sum(i.file_size for i in infos)/1048576, sum(i.compress_size for i in infos)/1048576, apps))
g = collections.OrderedDict()
for i in infos:
    p = i.filename.split('/')[2:]
    k = p[0] if len(p) == 1 else (p[0] + '/' + p[1] if p[0] in ('Frameworks', 'ExportedGame', 'dm') and len(p) > 2 else p[0])
    if p[0] == 'ExportedGame' and len(p) > 3 and p[1] in ('extract', 'documents', 'shaders'): k = '/'.join(p[:3])
    a = g.setdefault(k, [0, 0]); a[0] += 1; a[1] += i.file_size
print('  layout (files, MB) under Payload/<app>.app/:')
for k, (n, s) in sorted(g.items(), key=lambda kv: -kv[1][1])[:44]: print('    %-62s %6d  %10.2f' % (k, n, s/1048576))
PY
W="$(mktemp -d)"; trap '/bin/rm -rf -- "$W"' EXIT
python3 -I - "$IPA" "$W" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1]); out = sys.argv[2]
for n in z.namelist():
    if n.endswith(('/Info.plist', '/manifest.tsv', '/profile.tsv')) and n.count('/') <= 4 and 'profile-derived' not in n:
        z.extract(n, out)
PY
APP="$(ls -d "$W"/Payload/*.app 2>/dev/null | head -1)"
ENTRIES="$(unzip -Z1 "$IPA")"
[ -n "$APP" ] || { fail "no Payload/*.app"; exit 1; }
NAPP="$(printf '%s\n' "$ENTRIES" | grep -oE '^Payload/[^/]+\.app' | sort -u | wc -l | tr -d ' ')"
[ "$NAPP" = 1 ] && pass "exactly one Payload/<Name>.app: $(basename "$APP")" || fail "$NAPP apps under Payload/"
say "identity"
ID="$(plutil -extract CFBundleIdentifier raw "$APP/Info.plist" 2>/dev/null)"; EX="$(plutil -extract CFBundleExecutable raw "$APP/Info.plist" 2>/dev/null)"
echo "  CFBundleIdentifier $ID, executable $EX, display name '$(plutil -extract CFBundleDisplayName raw "$APP/Info.plist" 2>/dev/null)', CFBundleName '$(plutil -extract CFBundleName raw "$APP/Info.plist" 2>/dev/null)'"
case "$ID" in "$PREFIX".g*) pass "its own bundle id ($ID): installs beside the iBuilder app ($PREFIX)";; *) fail "bundle id $ID is not $PREFIX.g<title id>";; esac
[ "$ID" != "$PREFIX" ] && pass "not iBuilder's own id" || fail "the export has iBuilder's own bundle id"
plutil -extract CFBundleIcons.CFBundlePrimaryIcon.CFBundleIconFiles.0 raw "$APP/Info.plist" 2>/dev/null | grep -q GameIcon60x60 && pass "the game's icon (GameIcon60x60@2x/@3x.png) is named in CFBundleIcons" || fail "no game icon in Info.plist"
FWP="$APP/Frameworks/DmGameProbe.framework/Info.plist"
echo "  framework Info.plist: id $(plutil -extract CFBundleIdentifier raw "$FWP" 2>/dev/null), exec $(plutil -extract CFBundleExecutable raw "$FWP" 2>/dev/null), MinimumOSVersion $(plutil -extract MinimumOSVersion raw "$FWP" 2>/dev/null)"
case "$(plutil -extract CFBundleIdentifier raw "$FWP" 2>/dev/null)" in "$ID".DmGameProbe) pass "the framework's id is <app id>.DmGameProbe";; *) fail "framework id is not <app id>.DmGameProbe";; esac
say "manifest (ExportedGame/manifest.tsv)"
sed 's/^/  /' "$APP/ExportedGame/manifest.tsv" 2>/dev/null | head -20
say "checks"
has() { printf '%s\n' "$ENTRIES" | grep -qE "$1"; }
has "^Payload/[^/]+\.app/$EX\$" && pass "the executable $EX is in the bundle" || fail "no executable $EX"
has '^Payload/[^/]+\.app/Frameworks/DmGameProbe\.framework/DmGameProbe$' && pass "Frameworks/DmGameProbe.framework/DmGameProbe (the assembled game)" || fail "no DmGameProbe Mach-O"
has '^Payload/[^/]+\.app/Frameworks/DmGameProbe\.framework/Info\.plist$' && pass "Frameworks/DmGameProbe.framework/Info.plist" || fail "no framework Info.plist"
has '^Payload/[^/]+\.app/ExportedGame/manifest\.tsv$' && pass "ExportedGame/manifest.tsv (player mode marker)" || fail "no manifest.tsv"
has '^Payload/[^/]+\.app/ExportedGame/extract/flat/main\.patched\.bin$' && pass "ExportedGame/extract/flat/main.patched.bin (rewritten modules)" || fail "no extract/flat/main.patched.bin"
has '^Payload/[^/]+\.app/ExportedGame/shaders/[0-9A-F]{16}/b-[0-9a-f]{12}\.img$' && pass "ExportedGame/shaders/<id>/b-<chain>.img (the shader library)" || fail "no shader library image"
has '^Payload/[^/]+\.app/ExportedGame/documents/profile\.tsv$' && pass "ExportedGame/documents/profile.tsv" || fail "no documents/profile.tsv"
if [ "$KIND" = full ]; then has '^Payload/[^/]+\.app/ExportedGame/extract/RomFS\.img$' && pass "full export: ExportedGame/extract/RomFS.img inside" || fail "full export without RomFS.img"
else has '/ExportedGame/extract/RomFS\.img$' && fail "light export carries RomFS.img" || pass "light export: no RomFS.img inside (the .romfs travels beside the .ipa)"; grep -q '^romfs_file' "$APP/ExportedGame/manifest.tsv" && pass "manifest names romfs_file" || fail "manifest has no romfs_file"; fi
has '(\.py|\.pyc|\.fwork)$|/py/|/pyext/|Python\.framework|lib-dynload|/shaderengine/' && fail "Python pieces in the export" || pass "no Python anywhere in the exported .ipa (no .py/.pyc/.fwork, no py/ pyext/ shaderengine/ Python.framework)"
if printf '%s\n' "$ENTRIES" | grep -E '^Payload/[^/]+\.app/(_CodeSignature/|Frameworks/DmGameProbe\.framework/_CodeSignature/)' >/dev/null; then fail "_CodeSignature at the bundle root or in DmGameProbe.framework"; else pass "no _CodeSignature at the bundle root nor in DmGameProbe.framework (the installer signs; the export skips them)"; fi
NCS="$(printf '%s\n' "$ENTRIES" | grep -c '/_CodeSignature/' || true)"; echo "  nested _CodeSignature entries copied from the app's own frameworks: $NCS (a simulator bundle signs each nested framework ad hoc; the device bundle signs only its executable: 0)"
has '/embedded\.mobileprovision$' && fail "a provisioning profile in the archive" || pass "no embedded.mobileprovision"
has '/ExportedGame/ExportedGame/' && fail "ExportedGame nested" || pass "no nested ExportedGame"
has '^Payload/[^/]+\.app/dm/games/settings\.tsv$' && pass "the bundle's own data comes along (dm/games, host-prebuilt.dylib): the app copy is whole" || fail "dm/games missing"
has '^Payload/[^/]+\.app/host-prebuilt\.dylib$' && pass "host-prebuilt.dylib" || fail "no host-prebuilt.dylib"
mkdir -p "$W/exe"; unzip -q -o -j "$IPA" "Payload/$(basename "$APP")/$EX" -d "$W/exe" 2>/dev/null
if [ -f "$W/exe/$EX" ]; then
  codesign -dv "$W/exe/$EX" 2>&1 | grep -q 'Authority=' && fail "the executable carries a signing authority" || pass "the executable carries no identity (ad hoc / unsigned: SideStore re-signs)"
  echo "  executable signature: $(codesign -dv "$W/exe/$EX" 2>&1 | grep -E 'flags|Signature' | tr '\n' ' ')"
fi
[ "$BAD" = 0 ] && echo "ALL EXPORT CHECKS PASSED" || echo "SOME EXPORT CHECKS FAILED"; exit "$BAD"
