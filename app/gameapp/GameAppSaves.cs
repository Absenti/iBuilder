// gameapp/GameAppSaves.cs: a game's save data, kept as copies (inspired by Cartouche's saves sheet and JKSV).
//
// Where the engine keeps a game's saves: DM_SAVE = Documents/games/<title id>/save (the save's own files at its root,
// as on a Switch) and the device save beside it, <DM_SAVE>_device (hle_fs.c). A copy is one zip:
//   Documents/SaveBackups/<Game> [<title id>]/<yyyy-MM-dd HH.mm.ss> <kind>.zip
// with save/... and save_device/... inside, and backup.txt (title id, game, kind, when). Kinds: auto (before each
// launch: the ten newest are kept), manual (kept until deleted), before-restore and before-import (the state a restore
// or an import replaced: never lost by one tap). The folder is in Files (Documents is shared), so copies can be moved
// off the phone by hand too.
//
// JKSV (the save manager on a Switch) backs a save up as a zip of the save's files at its root: export writes one, and
// import takes one (or one of these copies) in place of the current save, after a before-import copy.
//
// No UIKit here: gameapp/tests/GameAppUi.Tests runs all of it on the Mac against temporary folders.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Text;

public static class GameSaves
{
    public const int KeepAuto = 10;
    public const string Root = "SaveBackups";

    public sealed record Copy(string Path, DateTime When, string Kind, long Size);

    public enum ArchiveKind { Ours, Jksv, Empty }

    public static string SaveDir(string documents, string titleId) => System.IO.Path.Combine(documents, "games", titleId, "save");

    public static string DeviceDir(string documents, string titleId) => SaveDir(documents, titleId) + "_device";

    /// Documents/SaveBackups/<Game> [<id>]: the game's name kept readable in Files, the title id keeping two games apart.
    public static string CopiesDir(string documents, string titleId, string gameName) =>
        System.IO.Path.Combine(documents, Root, $"{GameAppUi.ExportFileName(gameName)} [{titleId}]");

    public static string CopyFileName(DateTime when, string kind) =>
        when.ToString("yyyy-MM-dd HH.mm.ss", CultureInfo.InvariantCulture) + " " + kind + ".zip";

    /// (when, kind) from a copy's file name, or null for a file that is not one.
    public static (DateTime When, string Kind)? ParseCopyName(string fileName)
    {
        if (fileName == null || !fileName.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)) return null;
        var name = System.IO.Path.GetFileNameWithoutExtension(fileName);
        if (name.Length < 21) return null;
        if (!DateTime.TryParseExact(name.Substring(0, 19), "yyyy-MM-dd HH.mm.ss", CultureInfo.InvariantCulture,
                                    DateTimeStyles.None, out var when)) return null;
        var kind = name.Length > 20 ? name.Substring(20) : "manual";
        return (when, kind);
    }

    public static string KindLabel(string kind) => kind switch
    {
        "auto" => "before a launch",
        "manual" => "saved by you",
        "before-restore" => "replaced by a restore",
        "before-import" => "replaced by an import",
        _ => kind,
    };

    /// The game's copies, newest first.
    public static List<Copy> List(string documents, string titleId, string gameName)
    {
        var dir = CopiesDir(documents, titleId, gameName);
        var list = new List<Copy>();
        if (!Directory.Exists(dir)) return list;
        foreach (var f in Directory.EnumerateFiles(dir, "*.zip"))
        {
            var p = ParseCopyName(System.IO.Path.GetFileName(f));
            if (p == null) continue;
            list.Add(new Copy(f, p.Value.When, p.Value.Kind, new FileInfo(f).Length));
        }
        return list.OrderByDescending(c => c.When).ThenByDescending(c => c.Path, StringComparer.Ordinal).ToList();
    }

    /// Bytes of the game's save now (both folders), and when it last changed (null: no save yet).
    public static (long Bytes, DateTime? Changed) Current(string documents, string titleId)
    {
        long bytes = 0; DateTime? changed = null;
        foreach (var d in new[] { SaveDir(documents, titleId), DeviceDir(documents, titleId) })
        {
            if (!Directory.Exists(d)) continue;
            foreach (var f in Directory.EnumerateFiles(d, "*", SearchOption.AllDirectories))
            {
                var i = new FileInfo(f);
                bytes += i.Length;
                if (changed == null || i.LastWriteTime > changed) changed = i.LastWriteTime;
            }
        }
        return (bytes, changed);
    }

    /// A copy of the save now, of `kind`. Null when there is nothing to copy (the game has not saved yet). Auto copies
    /// beyond the ten newest are removed; manual and before-* copies stay until the player deletes them.
    public static string Create(string documents, string titleId, string gameName, string kind, DateTime now)
    {
        var save = SaveDir(documents, titleId);
        var device = DeviceDir(documents, titleId);
        bool any = (Directory.Exists(save) && Directory.EnumerateFileSystemEntries(save).Any())
                   || (Directory.Exists(device) && Directory.EnumerateFileSystemEntries(device).Any());
        if (!any) return null;
        var dir = CopiesDir(documents, titleId, gameName);
        Directory.CreateDirectory(dir);
        var path = System.IO.Path.Combine(dir, CopyFileName(now, kind));
        for (int n = 2; File.Exists(path); n++) path = System.IO.Path.Combine(dir, CopyFileName(now, kind + " " + n));
        var part = path + ".part";
        using (var zip = ZipFile.Open(part, ZipArchiveMode.Create))
        {
            AddTree(zip, save, "save/");
            AddTree(zip, device, "save_device/");
            var meta = zip.CreateEntry("backup.txt");
            using var w = new StreamWriter(meta.Open(), new UTF8Encoding(false));
            w.Write($"title_id\t{titleId}\ngame\t{gameName}\nkind\t{kind}\nwhen\t{now.ToString("o", CultureInfo.InvariantCulture)}\n");
        }
        File.Move(part, path);
        if (kind == "auto")
            foreach (var old in List(documents, titleId, gameName).Where(c => c.Kind == "auto").Skip(KeepAuto))
                try { File.Delete(old.Path); } catch (Exception) { }
        return path;
    }

    private static void AddTree(ZipArchive zip, string dir, string prefix)
    {
        if (!Directory.Exists(dir)) return;
        foreach (var f in Directory.EnumerateFiles(dir, "*", SearchOption.AllDirectories))
            zip.CreateEntryFromFile(f, prefix + System.IO.Path.GetRelativePath(dir, f).Replace(System.IO.Path.DirectorySeparatorChar, '/'),
                                    CompressionLevel.Fastest);
    }

    /// What a zip holds: one of these copies (save/ and save_device/), a JKSV backup (the save's files at its root, as
    /// JKSV writes it), or nothing usable. A single top folder is NOT taken for a wrapper: a save's own files can sit in
    /// one folder (stripping it lost them, caught by the tests), so a JKSV zip is laid in as it is.
    public static ArchiveKind Classify(IEnumerable<string> entryNames, out string jksvRoot)
    {
        jksvRoot = "";
        var files = entryNames.Where(n => !string.IsNullOrEmpty(n) && !n.EndsWith("/")).ToList();
        if (files.Count == 0 || files.All(n => n == "backup.txt")) return ArchiveKind.Empty;
        if (files.Any(n => n.StartsWith("save/") || n.StartsWith("save_device/"))) return ArchiveKind.Ours;
        return ArchiveKind.Jksv;
    }

    /// Puts a copy (or a JKSV backup) in place of the current save. The current save is first kept as a copy of
    /// `beforeKind` (before-restore / before-import), so a restore can always be undone. Returns that copy's path
    /// (null when there was no save to keep). Never call while the game runs: its files are open.
    public static string Restore(string documents, string titleId, string gameName, string zipPath, string beforeKind, DateTime now)
    {
        string kept;
        ArchiveKind kind; string root;
        using (var probe = ZipFile.OpenRead(zipPath))
            kind = Classify(probe.Entries.Select(e => e.FullName), out root);
        if (kind == ArchiveKind.Empty) throw new InvalidDataException("this zip holds no save");
        kept = Create(documents, titleId, gameName, beforeKind, now);
        var save = SaveDir(documents, titleId);
        var device = DeviceDir(documents, titleId);
        var staging = save + ".restore";
        if (Directory.Exists(staging)) Directory.Delete(staging, true);
        Directory.CreateDirectory(staging);
        using (var zip = ZipFile.OpenRead(zipPath))
            foreach (var e in zip.Entries)
            {
                if (e.FullName.EndsWith("/") || e.FullName == "backup.txt") continue;
                string rel;
                if (kind == ArchiveKind.Ours)
                    rel = e.FullName.StartsWith("save_device/") ? "device/" + e.FullName.Substring(12)
                        : e.FullName.StartsWith("save/") ? "save/" + e.FullName.Substring(5) : null;
                else
                    rel = e.FullName.StartsWith(root) ? "save/" + e.FullName.Substring(root.Length) : null;
                if (rel == null) continue;
                var dest = System.IO.Path.GetFullPath(System.IO.Path.Combine(staging, rel));
                if (!dest.StartsWith(System.IO.Path.GetFullPath(staging) + System.IO.Path.DirectorySeparatorChar))
                    throw new InvalidDataException("this zip names a file outside the save: " + e.FullName);
                Directory.CreateDirectory(System.IO.Path.GetDirectoryName(dest));
                e.ExtractToFile(dest, true);
            }
        // Swap in: the save folder, and the device save only when the copy carries one (a JKSV backup has none).
        ReplaceDir(save, System.IO.Path.Combine(staging, "save"));
        if (Directory.Exists(System.IO.Path.Combine(staging, "device"))) ReplaceDir(device, System.IO.Path.Combine(staging, "device"));
        Directory.Delete(staging, true);
        return kept;
    }

    private static void ReplaceDir(string target, string source)
    {
        if (Directory.Exists(target)) Directory.Delete(target, true);
        if (Directory.Exists(source)) Directory.Move(source, target);
        else Directory.CreateDirectory(target);
    }

    /// A zip JKSV takes: the save's own files at its root (the account save; JKSV keeps device saves apart).
    public static string ExportJksv(string documents, string titleId, string gameName, DateTime now)
    {
        var save = SaveDir(documents, titleId);
        if (!Directory.Exists(save) || !Directory.EnumerateFileSystemEntries(save).Any()) return null;
        var dir = CopiesDir(documents, titleId, gameName);
        Directory.CreateDirectory(dir);
        var path = System.IO.Path.Combine(dir, "JKSV " + now.ToString("yyyy-MM-dd HH.mm.ss", CultureInfo.InvariantCulture) + ".zip");
        if (File.Exists(path)) File.Delete(path);
        using var zip = ZipFile.Open(path, ZipArchiveMode.Create);
        AddTree(zip, save, "");
        return path;
    }
}
