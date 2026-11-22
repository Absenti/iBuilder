#!/usr/bin/env python3
"""Patch of the engine's dm/tools/nx_extract/Program.cs (a COPY, made by scripts/build-app.sh): a package that carries its own update is built WITH it.

Why (device log, 2026-11-19): Mario Kart 8 Deluxe imported as a "merged XCI" (base game + the 4.0.0 update + DLC in one XCI). The app passes `-` as the update argument unless the player
imported a separate update file, and the extractor then read the base game only: "leaving out 5 NCAs ... (another title in the package: an update on the cartridge)", display version 1.0.0.
That base build's `main` is a 32-bit AArch32 program, which nothing here can rewrite or run (the registered builds are 4.0.0 and 4.0.1). The extractor's own comment says a merged package
"is given as both the base game and the update"; the app never does that for a single file. This patch does it inside the extractor: when no update was given and the package itself holds
a Patch content meta of the base game's title (id | 0x800), that meta's NCAs are the update.

Nothing changes for a package without an update meta, or when an update file is given.
usage: nx_extract_merged_update.py <Program.cs>      (exit 1 and no write when an anchor is missing: the engine's source moved)
"""
import sys

OLD_OPEN = '''        var basePkg = OpenNsp(ks, a[1], ContentMetaType.Application, app); var updPkg = a[2] == "-" ? null : OpenNsp(ks, a[2], ContentMetaType.Patch, app | 0x800);
'''
NEW_OPEN = OLD_OPEN + '''        string updPath = a[2];   // iBuilderRust patch: the package that holds the update (a[1] itself when the update is merged into it)
        if (updPkg == null && a[2] == "-") {
            ulong? baseTitle = Pick(basePkg, NcaContentType.Program, progId)?.Header.TitleId;
            if (baseTitle != null && CarriesPatch(ks, a[1], baseTitle.Value | 0x800)) {
                updPkg = OpenNsp(ks, a[1], ContentMetaType.Patch, baseTitle.Value | 0x800); updPath = a[1];
                Console.WriteLine($"the package also carries the update of {baseTitle:x16} (a merged package): it is built in with the base game");
            }
        }
'''
OLD_FIT = 'updPkg == null ? null : MissingKey(ks, updPkg, a[2]) });'
NEW_FIT = 'updPkg == null ? null : MissingKey(ks, updPkg, updPath) });'
OLD_METAS = '''    // The content metas (*.cnmt in the Meta NCAs) of a package's NCAs
    static IEnumerable<Cnmt> Metas(List<Nca> ncas) {'''
NEW_METAS = '''    // iBuilderRust patch: whether the package holds a Patch content meta of this title ID (a cartridge dump with its update on it, a merged NSP). A package that cannot be read this way
    // answers false: the extraction proper reports why
    static bool CarriesPatch(KeySet ks, string path, ulong id) {
        try {
            var ncas = OpenNsp(ks, path);
            Fit(new[] { MissingKey(ks, ncas.FindAll(n => n.Header.ContentType == NcaContentType.Meta), path) });
            foreach (Cnmt cnmt in Metas(ncas)) if (cnmt.Type == ContentMetaType.Patch && cnmt.TitleId == id) return true;
        } catch (Exception) { }
        return false;
    }

''' + OLD_METAS


def main():
    p = sys.argv[1]
    s = open(p, encoding='utf-8').read()
    if 'CarriesPatch(' in s:
        print('already patched:', p)
        return 0
    for name, old in (('update argument', OLD_OPEN), ('key check', OLD_FIT), ('content metas', OLD_METAS)):
        if s.count(old) != 1:
            print(f'FAIL: the anchor for the {name} is not in {p} exactly once ({s.count(old)}): the engine source moved', file=sys.stderr)
            return 1
    s = s.replace(OLD_OPEN, NEW_OPEN, 1).replace(OLD_FIT, NEW_FIT, 1).replace(OLD_METAS, NEW_METAS, 1)
    open(p, 'w', encoding='utf-8').write(s)
    print('patched:', p)
    return 0


if __name__ == '__main__':
    sys.exit(main())
