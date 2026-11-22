// gameapp/GameAppMods.cs: mods and cheats in the Atmosphere layout, the app's side (docs/DECISIONS.md 89, 90).
//
// What this file does: reads the layouts people download (a .zip or a folder), keeps what belongs to THIS game in one
// canonical store, and keeps the player's choices. What it does not do: apply anything. The build chain does that, in Python
// (gameapp/mods.py: the exefs patches before stage 2, the RomFS files and the cheats at each boot), reading the same store:
//
//   Documents/games/<title id>/mods/<mod name>/{romfs/..., exefs/<name>.ips|.pchtxt, cheats/<build id>.txt}
//   Documents/games/<title id>/mods.tsv      the choices (mod, patch and cheat rows, tab separated)
//   Documents/games/<title id>/mods.applied  what the last rewrite saw (the exefs signature), for "Rebuild to apply"
//   Documents/games/<title id>/build_ids.txt the NSOs' build ids, written by the chain, for the import's filter
//
// The layouts recognised (a folder in a path is matched without regard to case):
//   Atmosphere     [atmosphere/]contents/<title id>/romfs/..., .../exefs/<build id>.ips, .../cheats/<build id>.txt
//                  [atmosphere/]exefs_patches/<name>/<build id>.ips (or .pchtxt)
//   Ryujinx/yuzu   mods/contents/<title id>/<mod name>/romfs|exefs|cheats/..., load/<title id>/<mod name>/romfs|exefs|cheats/...
//   loose          <mod name>/romfs|exefs|cheats/... at any depth, a .pchtxt or .ips anywhere, <build id>.txt under a cheats folder
// A title id in the path keeps the entry only for this game (its base id, or its update's: base + 0x800); an entry with none is
// kept. Once the game has been built the NSOs' build ids are known: an .ips, a cheat file or a .pchtxt that names none of them
// is another version of the game's and is left out (and said).
//
// No UIKit here: gameapp/tests/GameAppUi.Tests runs all of it on the Mac against temporary folders.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;

public static class GameMods
{
    public sealed record Entry(string Path, Func<Stream> Open);

    /// One stored mod, as the sheet lists it. Patches are a .pchtxt's @enabled / @disabled blocks (name, on now, in the file's own
    /// order); Cheats are the [sections] of its cheat files (name, on now).
    public sealed record Mod(string Name, bool Enabled, int RomfsFiles, int ExefsFiles, int CheatFiles,
                             List<(string Name, bool On)> Patches, List<(string Name, bool On)> Cheats, long Bytes);

    public sealed record ImportResult(List<string> Mods, int Kept, List<string> Notes)
    {
        public bool Any => Kept > 0;
    }

    public sealed class State
    {
        public Dictionary<string, bool> Mod = new Dictionary<string, bool>();
        public Dictionary<(string Mod, string Name), bool> Patch = new Dictionary<(string, string), bool>();
        public Dictionary<(string Mod, string Name), bool> Cheat = new Dictionary<(string, string), bool>();
    }

    // ---- the store ---------------------------------------------------------------------------------------------------

    public static string GameDir(string documents, string titleId) => Path.Combine(documents, "games", titleId);
    public static string ModsDir(string documents, string titleId) => Path.Combine(GameDir(documents, titleId), "mods");
    public static string StatePath(string documents, string titleId) => Path.Combine(GameDir(documents, titleId), "mods.tsv");
    public static string AppliedPath(string documents, string titleId) => Path.Combine(GameDir(documents, titleId), "mods.applied");
    public static string BuildIdsPath(string documents, string titleId) => Path.Combine(GameDir(documents, titleId), "build_ids.txt");

    public static State ReadState(string documents, string titleId)
    {
        var s = new State();
        string text;
        try { text = File.ReadAllText(StatePath(documents, titleId), Encoding.UTF8); } catch { return s; }
        foreach (var line in text.Split('\n'))
        {
            var c = line.TrimEnd('\r').Split('\t');
            if (c[0] == "mod" && c.Length >= 3) s.Mod[c[1]] = c[2].Trim() == "1";
            else if (c[0] == "patch" && c.Length >= 4) s.Patch[(c[1], c[2])] = c[3].Trim() == "1";
            else if (c[0] == "cheat" && c.Length >= 4) s.Cheat[(c[1], c[2])] = c[3].Trim() == "1";
        }
        return s;
    }

    public static void WriteState(string documents, string titleId, State s)
    {
        var sb = new StringBuilder();
        foreach (var kv in s.Mod.OrderBy(k => k.Key, StringComparer.Ordinal)) sb.Append($"mod\t{kv.Key}\t{(kv.Value ? 1 : 0)}\n");
        foreach (var kv in s.Patch.OrderBy(k => k.Key.Mod, StringComparer.Ordinal).ThenBy(k => k.Key.Name, StringComparer.Ordinal))
            sb.Append($"patch\t{kv.Key.Mod}\t{kv.Key.Name}\t{(kv.Value ? 1 : 0)}\n");
        foreach (var kv in s.Cheat.OrderBy(k => k.Key.Mod, StringComparer.Ordinal).ThenBy(k => k.Key.Name, StringComparer.Ordinal))
            sb.Append($"cheat\t{kv.Key.Mod}\t{kv.Key.Name}\t{(kv.Value ? 1 : 0)}\n");
        Directory.CreateDirectory(GameDir(documents, titleId));
        var tmp = StatePath(documents, titleId) + ".tmp";
        File.WriteAllText(tmp, sb.ToString(), new UTF8Encoding(false));
        File.Move(tmp, StatePath(documents, titleId), true);   // never half written: the boot reads it
    }

    public static void SetMod(string documents, string titleId, string mod, bool on)
    {
        var s = ReadState(documents, titleId); s.Mod[mod] = on; WriteState(documents, titleId, s);
    }

    public static void SetPatch(string documents, string titleId, string mod, string name, bool on)
    {
        var s = ReadState(documents, titleId); s.Patch[(mod, name)] = on; WriteState(documents, titleId, s);
    }

    public static void SetCheat(string documents, string titleId, string mod, string name, bool on)
    {
        var s = ReadState(documents, titleId); s.Cheat[(mod, name)] = on; WriteState(documents, titleId, s);
    }

    public static void Delete(string documents, string titleId, string mod)
    {
        var dir = Path.Combine(ModsDir(documents, titleId), mod);
        if (Directory.Exists(dir)) Directory.Delete(dir, true);
        var s = ReadState(documents, titleId);
        s.Mod.Remove(mod);
        foreach (var k in s.Patch.Keys.Where(k => k.Mod == mod).ToList()) s.Patch.Remove(k);
        foreach (var k in s.Cheat.Keys.Where(k => k.Mod == mod).ToList()) s.Cheat.Remove(k);
        WriteState(documents, titleId, s);
    }

    private static IEnumerable<string> Files(string dir) =>
        Directory.Exists(dir)
            ? Directory.EnumerateFiles(dir, "*", SearchOption.AllDirectories).Where(f => !System.IO.Path.GetFileName(f).StartsWith("."))
            : Enumerable.Empty<string>();

    /// The stored mods, by name; the first in this order wins a RomFS file two mods both carry.
    public static List<Mod> List(string documents, string titleId)
    {
        var root = ModsDir(documents, titleId);
        var list = new List<Mod>();
        if (!Directory.Exists(root)) return list;
        var state = ReadState(documents, titleId);
        var buildIds = BuildIds(documents, titleId);
        foreach (var dir in Directory.EnumerateDirectories(root).OrderBy(d => System.IO.Path.GetFileName(d), StringComparer.Ordinal))
        {
            var name = System.IO.Path.GetFileName(dir);
            if (name.StartsWith(".")) continue;
            var rom = Files(Path.Combine(dir, "romfs")).ToList();
            var exe = Files(Path.Combine(dir, "exefs")).ToList();
            var chf = Files(Path.Combine(dir, "cheats")).ToList();
            var patches = new List<(string, bool)>();
            foreach (var f in exe.OrderBy(f => f, StringComparer.Ordinal))
                if (f.EndsWith(".pchtxt", StringComparison.OrdinalIgnoreCase))
                    foreach (var (pname, def, bid) in PchBlocks(SafeText(f)))
                        if (buildIds.Count == 0 || bid == null || buildIds.Any(b => BidMatches(bid, b)))
                            patches.Add((pname, state.Patch.TryGetValue((name, pname), out var v) ? v : def));
            var cheats = new List<(string, bool)>();
            foreach (var f in chf.OrderBy(f => f, StringComparer.Ordinal))
                foreach (var cname in CheatSections(SafeText(f)))
                    if (!cheats.Any(c => c.Item1 == cname))
                        cheats.Add((cname, state.Cheat.TryGetValue((name, cname), out var v) && v));
            long bytes = 0;
            foreach (var f in rom.Concat(exe).Concat(chf)) { try { bytes += new FileInfo(f).Length; } catch { } }
            list.Add(new Mod(name, state.Mod.TryGetValue(name, out var on) && on, rom.Count, exe.Count, chf.Count, patches, cheats, bytes));
        }
        return list;
    }

    private static string SafeText(string path) { try { return File.ReadAllText(path); } catch { return ""; } }

    // ---- reading the files ---------------------------------------------------------------------------------------------

    private static readonly Regex Hex = new Regex("^[0-9A-Fa-f]+$");
    private static readonly Regex SectionLine = new Regex(@"^[ \t]*\[([^\]\r\n]+)\][ \t]*$", RegexOptions.Multiline);

    /// The names of the [sections] of a cheat file, in file order (a {Master} is always on and not listed): what mods.py reads.
    public static List<string> CheatSections(string text) =>
        SectionLine.Matches(text).Cast<Match>().Select(m => m.Groups[1].Value.Trim()).Where(n => n.Length > 0).ToList();

    /// The @enabled / @disabled blocks of a .pchtxt as (name, on by default, the @nsobid- it sits under), named the way mods.py names
    /// them: the comment above the block, else "patch N" counting every block of the file (an address line before any block makes an
    /// implicit enabled one, as it does there).
    public static List<(string Name, bool On, string Bid)> PchBlocks(string text)
    {
        var res = new List<(string, bool, string)>();
        string bid = null, comment = ""; bool open = false; int count = 0;
        foreach (var raw in Regex.Replace(text, @"/\*.*?\*/", "", RegexOptions.Singleline).Split('\n'))
        {
            var line = raw.Trim();
            if (line.Length == 0) continue;
            if (line.StartsWith("//"))
            {
                var note = line.Substring(2).Trim().Trim('-', '=', '*', '#', ' ').Trim();
                if (note.Length > 0) comment = note;
                continue;
            }
            var slash = line.IndexOf("//", StringComparison.Ordinal);
            if (slash >= 0) line = line.Substring(0, slash).Trim();
            if (line.StartsWith("@"))
            {
                var key = line.Substring(1).Split(new[] { ' ', '\t' }, 2, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault()?.ToLowerInvariant() ?? "";
                if (key.StartsWith("nsobid-")) { bid = key.Substring(7).ToUpperInvariant(); open = false; }
                else if (key == "enabled" || key == "disabled")
                {
                    count++; res.Add((comment.Length > 0 ? comment : "patch " + count, key == "enabled", bid)); comment = ""; open = true;
                }
                else if (key == "stop") { bid = null; open = false; }
                continue;
            }
            if (bid == null || open) continue;
            var parts = line.Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
            if (parts.Length < 2 || line.Contains('"')) continue;
            var addr = parts[0].Replace("0x", "").Replace("0X", ""); var val = string.Concat(parts.Skip(1));
            if (!Hex.IsMatch(addr) || !Hex.IsMatch(val) || val.Length % 2 != 0) continue;
            count++; res.Add((comment.Length > 0 ? comment : "patch " + count, true, bid)); comment = ""; open = true;
        }
        return res;
    }

    /// Does a file's build id (16 or more hex digits; a trailing run of zero bytes is dropped by some tools) name an NSO whose id is
    /// `nsoBid` (hex)? The same rule as mods.py bid_matches.
    public static bool BidMatches(string wanted, string nsoBid)
    {
        if (wanted == null || wanted.Length < 16 || !Hex.IsMatch(wanted)) return false;
        var w = wanted.ToUpperInvariant().TrimEnd('0');
        return w.Length > 0 && nsoBid.ToUpperInvariant().StartsWith(w, StringComparison.Ordinal);
    }

    /// The build ids the chain recorded (the NSOs of this game's ExeFS), full hex; empty before the first build.
    public static List<string> BuildIds(string documents, string titleId)
    {
        try
        {
            return File.ReadAllLines(BuildIdsPath(documents, titleId)).Select(l => l.Split('\t')).Where(c => c.Length >= 2 && Hex.IsMatch(c[1]))
                .Select(c => c[1].ToUpperInvariant()).ToList();
        }
        catch { return new List<string>(); }
    }

    // ---- "Rebuild to apply" ---------------------------------------------------------------------------------------------

    /// What the exefs patches of the enabled mods are, as text: each enabled mod's exefs files with their SHA-256, then the
    /// choices of its patch blocks. mods.py computes the same text (exefs_signature) and writes it to mods.applied after a rewrite.
    public static string ExefsSignature(string documents, string titleId)
    {
        var state = ReadState(documents, titleId);
        var lines = new List<string>();
        var root = ModsDir(documents, titleId);
        if (!Directory.Exists(root)) return "";
        var names = Directory.EnumerateDirectories(root).Select(System.IO.Path.GetFileName).Where(n => !n.StartsWith("."))
            .Where(n => state.Mod.TryGetValue(n, out var on) && on).OrderBy(n => n, StringComparer.Ordinal);
        foreach (var m in names)
        {
            var exefs = Path.Combine(root, m, "exefs");
            foreach (var rel in Files(exefs).Select(f => System.IO.Path.GetRelativePath(exefs, f).Replace('\\', '/')).OrderBy(r => r, StringComparer.Ordinal))
            {
                using var fs = File.OpenRead(Path.Combine(exefs, rel));
                lines.Add($"file\t{m}\t{rel}\t{Convert.ToHexString(SHA256.HashData(fs)).ToLowerInvariant()}");
            }
            foreach (var kv in state.Patch.Where(k => k.Key.Mod == m).OrderBy(k => k.Key.Name, StringComparer.Ordinal))
                lines.Add($"patch\t{m}\t{kv.Key.Name}\t{(kv.Value ? 1 : 0)}");
        }
        return string.Join("\n", lines);
    }

    /// The enabled mods' exefs patches differ from what the last rewrite applied: the game has to be rebuilt for them.
    public static bool ExefsChangedSinceBuild(string documents, string titleId)
    {
        string applied = "";
        try { applied = File.ReadAllText(AppliedPath(documents, titleId), Encoding.UTF8); } catch { }
        return applied != ExefsSignature(documents, titleId);
    }

    // ---- importing ---------------------------------------------------------------------------------------------------

    private static readonly string[] Generic = { "atmosphere", "contents", "mods", "load", "mod", "exefs_patches", "titles" };
    private static readonly Regex TitleSeg = new Regex("^[0-9A-Fa-f]{16}$");

    /// A mod's folder name made safe: no path separators, no control characters, not empty, not hidden, at most 64 characters.
    public static string SafeName(string name)
    {
        var sb = new StringBuilder();
        foreach (var ch in name ?? "") sb.Append(char.IsControl(ch) || "/\\:*?\"<>|".IndexOf(ch) >= 0 ? '_' : ch);
        var s = sb.ToString().Trim().TrimStart('.').Trim();
        if (s.Length > 64) s = s.Substring(0, 64).Trim();
        return s.Length == 0 ? "Mod" : s;
    }

    /// The entries of a .zip: files only, `__MACOSX`, `.DS_Store` and `._` files left out, a path that leaves the archive refused.
    public static List<Entry> ZipEntries(ZipArchive zip)
    {
        var list = new List<Entry>();
        foreach (var e in zip.Entries)
        {
            var p = e.FullName.Replace('\\', '/');
            if (p.EndsWith("/") || e.Name.Length == 0) continue;
            var seg = p.Split('/');
            if (seg.Any(s => s == ".." || s == ".") || p.StartsWith("/")) continue;
            if (seg.Contains("__MACOSX") || e.Name == ".DS_Store" || e.Name.StartsWith("._")) continue;
            var entry = e;
            list.Add(new Entry(p, () => entry.Open()));
        }
        return list;
    }

    public static List<Entry> FolderEntries(string folder)
    {
        var list = new List<Entry>();
        foreach (var f in Directory.EnumerateFiles(folder, "*", SearchOption.AllDirectories))
        {
            var rel = System.IO.Path.GetRelativePath(folder, f).Replace('\\', '/');
            var name = System.IO.Path.GetFileName(f);
            if (rel.Split('/').Contains("__MACOSX") || name == ".DS_Store" || name.StartsWith("._")) continue;
            var path = f;
            list.Add(new Entry(rel, () => File.OpenRead(path)));
        }
        return list;
    }

    private sealed record Placed(string Mod, string Kind, string Rel, Entry Source, string TitleId);

    /// What a path is: (mod name or null for "the source's name", kind romfs|exefs|cheats, the path inside the kind's folder, a title
    /// id in the path or null), or null when the layouts above do not recognise it.
    private static (string Mod, string Kind, string Rel, string Tid)? Classify(string path)
    {
        var seg = path.Split('/');
        string tid = seg.Take(seg.Length - 1).FirstOrDefault(s => TitleSeg.IsMatch(s))?.ToUpperInvariant();
        int a = Array.FindIndex(seg, 0, seg.Length - 1, s => s.Equals("romfs", StringComparison.OrdinalIgnoreCase) || s.Equals("exefs", StringComparison.OrdinalIgnoreCase)
                                                             || s.Equals("cheats", StringComparison.OrdinalIgnoreCase));
        var file = seg[seg.Length - 1];
        if (a >= 0)
        {
            var kind = seg[a].ToLowerInvariant();
            string mod = null;
            if (a > 0 && !TitleSeg.IsMatch(seg[a - 1]) && !Generic.Contains(seg[a - 1].ToLowerInvariant())) mod = seg[a - 1];
            var rel = string.Join("/", seg.Skip(a + 1));
            if (kind == "cheats" && !(file.EndsWith(".txt", StringComparison.OrdinalIgnoreCase) && BidOfName(file) != null)) return null;
            if (kind == "exefs" && !(file.EndsWith(".ips", StringComparison.OrdinalIgnoreCase) || file.EndsWith(".pchtxt", StringComparison.OrdinalIgnoreCase))) return null;
            return (mod, kind, rel, tid);
        }
        // exefs_patches/<name>/<build id>.ips, and a loose .ips or .pchtxt
        int ep = Array.FindIndex(seg, s => s.Equals("exefs_patches", StringComparison.OrdinalIgnoreCase));
        if (file.EndsWith(".ips", StringComparison.OrdinalIgnoreCase) || file.EndsWith(".pchtxt", StringComparison.OrdinalIgnoreCase))
        {
            string mod = ep >= 0 && ep + 2 < seg.Length ? seg[ep + 1]
                       : seg.Length > 1 && !TitleSeg.IsMatch(seg[seg.Length - 2]) && !Generic.Contains(seg[seg.Length - 2].ToLowerInvariant()) ? seg[seg.Length - 2] : null;
            return (mod, "exefs", file, tid);
        }
        return null;
    }

    /// The 16 hex digits (upper case) a cheat file's name starts with, which is the build id, or null.
    public static string BidOfName(string fileName)
    {
        var stem = System.IO.Path.GetFileNameWithoutExtension(fileName);
        return stem.Length >= 16 && Hex.IsMatch(stem) ? stem.ToUpperInvariant() : null;
    }

    /// Keep what belongs to this game from `entries`, store it as mods named after the folders it came in (`sourceName` for files
    /// that name none) and turn each new mod on. Throws InvalidOperationException with the reason when nothing is kept.
    public static ImportResult Import(string documents, string titleId, IEnumerable<Entry> entries, string sourceName)
    {
        var update = (Convert.ToUInt64(titleId, 16) | 0x800UL).ToString("X16", CultureInfo.InvariantCulture);
        var known = BuildIds(documents, titleId);
        var placed = new List<Placed>();
        var otherTitles = new SortedSet<string>();
        int otherBuilds = 0, unrecognised = 0, ipsNoBid = 0;
        foreach (var e in entries)
        {
            var c = Classify(e.Path);
            if (c == null) { unrecognised++; continue; }
            if (c.Value.Tid != null && c.Value.Tid != titleId.ToUpperInvariant() && c.Value.Tid != update) { otherTitles.Add(c.Value.Tid); continue; }
            var (mod, kind, rel, tid) = c.Value;
            var file = rel.Split('/').Last();
            if (kind == "exefs" && file.EndsWith(".ips", StringComparison.OrdinalIgnoreCase) && BidOfName(file) == null) { ipsNoBid++; continue; }
            if (known.Count > 0)
            {
                string bid = kind == "cheats" || (kind == "exefs" && file.EndsWith(".ips", StringComparison.OrdinalIgnoreCase)) ? BidOfName(file) : null;
                bool keep = true;
                if (bid != null) keep = known.Any(k => BidMatches(bid, k));
                else if (kind == "exefs")   // a .pchtxt: kept when some @nsobid- in it is one of ours
                {
                    string text; using (var s = e.Open()) using (var r = new StreamReader(s)) text = r.ReadToEnd();
                    keep = PchBlocks(text).Any(b => b.Bid == null || known.Any(k => BidMatches(b.Bid, k)));
                }
                if (!keep) { otherBuilds++; continue; }
            }
            placed.Add(new Placed(SafeName(mod ?? sourceName), kind, rel, e, tid));
        }
        var notes = new List<string>();
        if (otherTitles.Count > 0) notes.Add($"left out: files for other games ({string.Join(", ", otherTitles.Take(3))}{(otherTitles.Count > 3 ? ", ..." : "")})");
        if (otherBuilds > 0) notes.Add($"left out: {otherBuilds} file(s) for another version of the game (their build id is not this game's)");
        if (ipsNoBid > 0) notes.Add($"left out: {ipsNoBid} .ips file(s) not named by a build id (the game cannot be told from the name)");
        if (placed.Count == 0)
        {
            var why = notes.Count > 0 ? " " + string.Join("; ", notes) + "." : "";
            throw new InvalidOperationException("Nothing in it is a mod for this game." + why + (unrecognised > 0 && notes.Count == 0
                ? " Expected atmosphere/contents/<title id>/romfs, exefs or cheats, a mods/<name>/romfs|exefs|cheats folder, or a .pchtxt / .ips patch." : ""));
        }
        var root = ModsDir(documents, titleId);
        var state = ReadState(documents, titleId);
        var names = new List<string>();
        foreach (var group in placed.GroupBy(p => p.Mod))
        {
            var dir = Path.Combine(root, group.Key);
            if (Directory.Exists(dir)) Directory.Delete(dir, true);   // the same name again: the new files replace the old ones
            foreach (var p in group)
            {
                var dest = Path.Combine(dir, p.Kind, p.Rel.Replace('/', System.IO.Path.DirectorySeparatorChar));
                Directory.CreateDirectory(System.IO.Path.GetDirectoryName(dest));
                using var src = p.Source.Open();
                using var dst = File.Create(dest);
                src.CopyTo(dst);
            }
            if (!state.Mod.ContainsKey(group.Key)) state.Mod[group.Key] = true;
            names.Add(group.Key);
        }
        WriteState(documents, titleId, state);
        return new ImportResult(names, placed.Count, notes);
    }

    /// Import from a .zip file or a folder on disk.
    public static ImportResult ImportPath(string documents, string titleId, string path)
    {
        if (Directory.Exists(path))
            return Import(documents, titleId, FolderEntries(path), System.IO.Path.GetFileName(path.TrimEnd('/', '\\')));
        if (!path.EndsWith(".zip", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException($"{System.IO.Path.GetFileName(path)} is not a .zip or a folder.");
        using var zip = ZipFile.OpenRead(path);
        return Import(documents, titleId, ZipEntries(zip), System.IO.Path.GetFileNameWithoutExtension(path));
    }
}
