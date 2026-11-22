#!/usr/bin/env python3
"""scripts/scrub-build-paths.py [--changed-list FILE] <file-or-folder>... -- blank the build machine's home directory out of binaries, in place and without changing any size.

Compilers write absolute source paths into the code they produce (`__FILE__`, panic locations, debug info, symbol-table object paths, a .pdb path in a .NET assembly). The Rust build maps
them away with --remap-path-prefix (scripts/build-ios-ffi.sh), but a static library built earlier by another project (zsign) or the host's object files still carry the path of the machine
that built them. A binary that is shared (a published IPA) must not tell who built it: every `/Users/<this user>` followed by a path separator (or the end of the string) becomes
`/Users/xxxx` of the same length (the user name is replaced by as many `x` as it has BYTES), so no offset, size or layout changes. Code signatures are NOT valid afterwards: the caller signs
again (scripts/build-app.sh does, after this runs; `--changed-list` names the files that were modified).

What is touched (review r5 M1, M2):
  * only `/Users/<name>` for the user's name (USER, getpass) and for $HOME when it is itself `/Users/<name>`; a $HOME outside /Users/ (/usr, /var/root, /) is refused with a warning, never
    blanked, because replacing a short or system path would damage unrelated strings (load-command paths such as /usr/lib/libSystem.B.dylib);
  * a name boundary is required after the name: the next byte must not be one of [A-Za-z0-9._-], so `/Users/alice2/` and `/Users/aliceX` (another user) are left alone;
  * everything is done on bytes, so a non-ASCII user name works.
A folder argument is walked: every regular file (not a symbolic link) is read, whatever its kind (Mach-O, .dll, .pdb, text).

Usage: scrub-build-paths.py [--changed-list FILE] PATH [PATH...]   prints one line per file given (per file changed for a folder).   Exit 1 if a file cannot be read or written, 2 on usage.
"""
import getpass
import os
import re
import sys

NAME_BYTES = rb"[A-Za-z0-9._-]"


def candidates():
    """The home directories to blank, as bytes `/Users/<name>` (name without a slash), and the warnings for what is refused."""
    raw = {os.path.expanduser("~"), os.environ.get("HOME", "")}
    user = os.environ.get("USER") or getpass.getuser()
    raw.add("/Users/" + user)
    good, warn = set(), []
    for h in raw:
        if not h:
            continue
        b = os.fsencode(h).rstrip(b"/")
        m = re.fullmatch(rb"/Users/([^/]+)", b)
        if m and len(m.group(1)) >= 1:
            good.add(b)
        else:
            warn.append(f"scrub-build-paths: {h!r} is not /Users/<name>: not blanked (a short or system path would damage other strings); verify_ipa.sh still looks for it")
    return sorted(good, key=len, reverse=True), warn


def blank_for(home):
    prefix, name = home.rsplit(b"/", 1)
    return prefix + b"/" + b"x" * len(name)


def build_patterns(homes):
    return [(re.compile(re.escape(h) + rb"(?!" + NAME_BYTES + rb")"), blank_for(h)) for h in homes]


def scrub_bytes(data, patterns):
    total = 0
    for rx, blank in patterns:
        data, n = rx.subn(lambda m, b=blank: b, data)
        total += n
    return data, total


def scrub_file(path, patterns):
    with open(path, "rb") as f:
        data = f.read()
    new, total = scrub_bytes(data, patterns)
    if total:
        assert len(new) == len(data)
        tmp = path + ".scrub.tmp"
        with open(tmp, "wb") as f:
            f.write(new)
        os.chmod(tmp, os.stat(path).st_mode)
        os.replace(tmp, path)
    return total


def main(argv):
    changed_list = None
    if argv[:1] == ["--changed-list"]:
        if len(argv) < 2:
            print(__doc__)
            return 2
        changed_list, argv = argv[1], argv[2:]
    if not argv:
        print(__doc__)
        return 2
    homes, warnings = candidates()
    for w in warnings:
        print(w, file=sys.stderr)
    patterns = build_patterns(homes)
    changed = []
    for arg in argv:
        if os.path.isdir(arg) and not os.path.islink(arg):
            files = []
            for dp, dns, fns in os.walk(arg):
                dns.sort()
                files += [os.path.join(dp, fn) for fn in sorted(fns)]
            folder = True
        else:
            files, folder = [arg], False
        nfiles = 0
        ntotal = 0
        for path in files:
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            try:
                total = scrub_file(path, patterns)
            except OSError as e:
                print(f"scrub-build-paths: cannot read or write {path}: {e}", file=sys.stderr)
                return 1
            nfiles += 1
            ntotal += total
            if total:
                changed.append(path)
                if folder:
                    print(f"scrub-build-paths: {os.path.relpath(path, arg)}: {total} home-directory string(s) blanked")
            if not folder:
                print(f"scrub-build-paths: {os.path.basename(path)}: {total} home-directory string(s) blanked")
        if folder:
            print(f"scrub-build-paths: {arg}: {nfiles} file(s) read, {ntotal} home-directory string(s) blanked")
    if changed_list:
        try:
            with open(changed_list, "w", encoding="utf-8", errors="surrogateescape") as f:
                f.write("".join(p + "\n" for p in changed))
        except OSError as e:
            print(f"scrub-build-paths: cannot write {changed_list}: {e}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
