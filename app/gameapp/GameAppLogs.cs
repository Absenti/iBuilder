// The logs an app can hand over: the last 10 minutes of every log file, complete, in one zip.
//
// The log files are written by different hands (this app, the chain's Python, the shim's C, the guest's own stdout)
// and only this app's has lines it could time-stamp. So the window is kept the format-independent way: every 30 s
// the app writes down each log file's length and the time ("marks", Documents/gameapp.logmarks). The last 10
// minutes of a file are then everything from the length it had at the newest mark that is 10 minutes old or older,
// to its end: no line inside the window is cut and no byte cap sits in the middle. What is older may go.
// Plain C# with no UIKit, so GameAppUi.Tests runs it.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Text;

public static class LogWindow
{
    public static readonly TimeSpan Window = TimeSpan.FromMinutes(10);
    public static readonly TimeSpan MarkEvery = TimeSpan.FromSeconds(30);
    public const string MarksFile = "gameapp.logmarks";

    /// The ui log is trimmed to the window once it is this large (never inside the window).
    public const long UiLogTrimAbove = 64L << 20;

    private static readonly string[] Named = { "gameapp.ui.log", "pipeline.stages.log", "gameapp.chain.log", "gameapp.stage", "gameapp.guest.log" };

    /// Every log file of the app, as paths relative to Documents: the known ones, any other *.log in Documents, and
    /// what is in Documents/logs. Only files that exist.
    public static List<string> LogFiles(string documents)
    {
        var found = new List<string>();
        foreach (var n in Named) if (File.Exists(Path.Combine(documents, n))) found.Add(n);
        try
        {
            foreach (var f in Directory.GetFiles(documents, "*.log").Select(Path.GetFileName).OrderBy(n => n, StringComparer.Ordinal))
                if (!found.Contains(f)) found.Add(f);
            var sub = Path.Combine(documents, "logs");
            if (Directory.Exists(sub))
                foreach (var f in Directory.GetFiles(sub).Select(Path.GetFileName).OrderBy(n => n, StringComparer.Ordinal))
                    found.Add("logs/" + f);
        }
        catch (Exception) { }
        return found;
    }

    public static string MarkLine(DateTime utc, string name, long length) =>
        string.Create(CultureInfo.InvariantCulture, $"{utc.Ticks}\t{name}\t{length}");

    public static List<(DateTime Utc, string Name, long Length)> ParseMarks(string text)
    {
        var all = new List<(DateTime, string, long)>();
        foreach (var line in (text ?? "").Split('\n'))
        {
            var p = line.TrimEnd('\r').Split('\t');
            if (p.Length == 3 && long.TryParse(p[0], NumberStyles.Integer, CultureInfo.InvariantCulture, out var t)
                && long.TryParse(p[2], NumberStyles.Integer, CultureInfo.InvariantCulture, out var len) && t > 0 && t <= DateTime.MaxValue.Ticks)
                all.Add((new DateTime(t, DateTimeKind.Utc), p[1], len));
        }
        return all;
    }

    /// Where the window of `name` starts: the length the file had at its newest mark that is `Window` old or older
    /// (never past the file's end: a file that was replaced shorter is kept whole). 0 when no mark is old enough.
    public static long StartOffset(IEnumerable<(DateTime Utc, string Name, long Length)> marks, string name, DateTime nowUtc, long length)
    {
        long start = 0; DateTime best = DateTime.MinValue;
        foreach (var m in marks)
            if (m.Name == name && nowUtc - m.Utc >= Window && m.Utc > best && m.Length <= length) { best = m.Utc; start = m.Length; }
        return start;
    }

    /// The marks worth keeping: everything inside the window, and per file the newest one just outside it.
    public static List<(DateTime Utc, string Name, long Length)> Prune(IEnumerable<(DateTime Utc, string Name, long Length)> marks, DateTime nowUtc)
    {
        var list = marks.ToList();
        var keep = list.Where(m => nowUtc - m.Utc < Window).ToList();
        foreach (var g in list.Where(m => nowUtc - m.Utc >= Window).GroupBy(m => m.Name))
            keep.Add(g.OrderByDescending(m => m.Utc).First());
        return keep.OrderBy(m => m.Utc).ToList();
    }

    private static string MarksPath(string documents) => Path.Combine(documents, MarksFile);

    private static List<(DateTime Utc, string Name, long Length)> ReadMarks(string documents)
    {
        try { return ParseMarks(File.ReadAllText(MarksPath(documents))); } catch (Exception) { return new(); }
    }

    private static void WriteMarks(string documents, IEnumerable<(DateTime Utc, string Name, long Length)> marks)
    {
        try
        {
            var text = string.Join("\n", marks.Select(m => MarkLine(m.Utc, m.Name, m.Length))) + "\n";
            File.WriteAllText(MarksPath(documents), text);
        }
        catch (Exception) { }
    }

    /// Writes down each log file's length now. Called every 30 s while the app is open, and at launch.
    public static void Mark(string documents, DateTime nowUtc)
    {
        lock (Gate) MarkLocked(documents, nowUtc);
    }

    private static readonly object Gate = new object();

    private static void MarkLocked(string documents, DateTime nowUtc)
    {
        var marks = ReadMarks(documents);
        foreach (var f in LogFiles(documents))
            try { marks.Add((nowUtc, f, new FileInfo(Path.Combine(documents, f)).Length)); } catch (Exception) { }
        WriteMarks(documents, Prune(marks, nowUtc));
    }

    /// Rewrites `name` (a file nothing else holds open: the ui log) from the start of its window to its end, and moves
    /// its marks with it. Nothing inside the window is cut. False when there was nothing to drop.
    public static bool TrimToWindow(string documents, string name, DateTime nowUtc)
    {
        lock (Gate)
        try
        {
            var path = Path.Combine(documents, name);
            long length = new FileInfo(path).Length;
            var marks = ReadMarks(documents);
            long start = StartOffset(marks, name, nowUtc, length);
            if (start <= 0) return false;
            var temp = path + ".trim";
            using (var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite))
            using (var output = new FileStream(temp, FileMode.Create, FileAccess.Write))
            {
                input.Seek(start, SeekOrigin.Begin);
                input.CopyTo(output);
            }
            File.Move(temp, path, true);
            var moved = marks.Where(m => m.Name != name || m.Length >= start).Select(m => m.Name == name ? (m.Utc, m.Name, m.Length - start) : m);
            WriteMarks(documents, moved);
            return true;
        }
        catch (Exception) { return false; }
    }

    /// "<Game> logs <date>.zip"
    public static string ZipName(string game, DateTime local) =>
        $"{GameAppUi.ExportFileName(game)} logs {local.ToString("yyyy-MM-dd HH-mm", CultureInfo.InvariantCulture)}.zip";

    /// Writes the zip of the last 10 minutes of every log file into `outDir` and returns its path (null: no log yet).
    /// A file is read as it is: a log being written grows past what was counted, never short of it.
    public static string BuildZip(string documents, string outDir, string game, DateTime nowLocal)
    {
        var nowUtc = nowLocal.ToUniversalTime();
        Mark(documents, nowUtc);   // the window ends here, with a mark of its own for the next export
        var files = LogFiles(documents);
        if (files.Count == 0) return null;
        var marks = ReadMarks(documents);
        Directory.CreateDirectory(outDir);
        var path = Path.Combine(outDir, ZipName(game, nowLocal));
        if (File.Exists(path)) File.Delete(path);
        var summary = new StringBuilder();
        summary.AppendLine($"{game}: the last {(int) Window.TotalMinutes} minutes of the logs, up to {nowLocal:yyyy-MM-dd HH:mm:ss}.");
        using (var stream = new FileStream(path, FileMode.Create, FileAccess.ReadWrite))
        using (var zip = new ZipArchive(stream, ZipArchiveMode.Create))
        {
            foreach (var f in files)
            {
                try
                {
                    using var input = new FileStream(Path.Combine(documents, f), FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
                    long start = StartOffset(marks, f, nowUtc, input.Length);
                    input.Seek(start, SeekOrigin.Begin);
                    var entry = zip.CreateEntry(f.Replace('/', '_'), CompressionLevel.Optimal);
                    using (var output = entry.Open()) input.CopyTo(output);
                    summary.AppendLine($"{f}: from byte {start} of {input.Length}" + (start == 0 ? " (all of it)" : ""));
                }
                catch (Exception e) { summary.AppendLine($"{f}: not read ({e.Message})"); }
            }
            var note = zip.CreateEntry("README.txt");
            using (var w = new StreamWriter(note.Open())) w.Write(summary.ToString());
        }
        return path;
    }
}
