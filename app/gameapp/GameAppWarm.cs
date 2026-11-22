// gameapp/GameAppWarm.cs: "Preparing the shaders" -- the game's Metal libraries compiled BEFORE the first play session, so that
// the system's Metal compile cache already holds them when the host asks (docs/DECISIONS.md 79, docs/PERF-RESEARCH.md items 3 and 9).
//
// Why it works: the host (paches/0007, nvn_mtl.m src_compile) compiles each library of a source pack at its first use with
//     [MTLDevice newLibraryWithSource: <the .metal file's text, as UTF-8> options: {mathMode = Relaxed, preserveInvariance = YES,
//      languageVersion = 4.0 ONLY when the text contains "vertex_value<", else the default; no macros}]
// and the system's compile cache, which works across launches of the SAME app (one library: 524.8 ms, then 1.4 ms), is keyed by the
// source text and those options. The host runs inside this app's process (a dylib), so this app's own call with the same text and the
// same options makes the same key. The options live in WarmCompile below: change them there and in nvn_mtl.m together, or the
// warm-up becomes a harmless no-op that costs heat and nothing else.
//
// Which sources: the pack's library, Documents/shaders/<id>/b-<chain>.img (a DMROMFS1 image holding <key>_b.metal, ...: read here in
// place, not unpacked), and the shaders the game learned while it ran (Documents/games/<id>/learned_shaders/b-<chain>/*.metal,
// paches/0008). A Mac-made pack holds .metallib files, nothing to compile: it is skipped. An exported player is another app (its own
// bundle identifier) with its own cache: it prepares its own at its first launch, from the pack inside its bundle.
//
// What is kept: Documents/games/<id>/warmed.tsv, one row per library compiled (name, a digest of its text, ms), under a header naming
// this app build and chain: another build of the app (the OS or the host may differ, the cache may be gone) starts it again.
//
// The pure parts (the image's index, the warmed.tsv rows, the options' decision) have no iOS types and are tested on a Mac
// (gameapp/tests/GameWarm.Tests, compiled with WARM_LOGIC_ONLY); the part below #if !WARM_LOGIC_ONLY needs the iOS SDK.

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
#if !WARM_LOGIC_ONLY
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Threading;
using Foundation;
using Metal;
using UIKit;
#endif

internal static class GameWarmLogic
{
    /// A file of the pack's image: its name, where its bytes start and how many.
    public sealed record ImageFile(string Name, long Offset, long Size);

    /// The files of a DMROMFS1 image (dm/tools/romfs_image.py: a 32-byte header "DMROMFS1", the index offset, size and entry count;
    /// 24-byte entries a, b, name offset, name length, type, flags; the names after them; entry 0 is the root, whose children are
    /// consecutive). The shader library is one flat folder. Null when the file is not an image this reads (never an exception: a
    /// damaged pack is the host's to complain about, the warm-up just has nothing to do).
    public static List<ImageFile> ReadImageIndex(string path)
    {
        try
        {
            using var f = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            var head = new byte[32];
            if (f.Read(head, 0, 32) != 32 || Encoding.ASCII.GetString(head, 0, 8) != "DMROMFS1") return null;
            long ioff = BitConverter.ToInt64(head, 8), isz = BitConverter.ToInt64(head, 16), n = BitConverter.ToInt64(head, 24);
            long length = f.Length;
            if (ioff < 32 || isz < 0 || n < 1 || n > 4_000_000 || ioff > length || isz > length - ioff || n * 24 > isz) return null;
            var index = new byte[isz];
            f.Position = ioff;
            for (int got = 0; got < index.Length;)
            {
                int r = f.Read(index, got, index.Length - got);
                if (r <= 0) return null;
                got += r;
            }
            long poolStart = n * 24;
            long first = BitConverter.ToInt64(index, 0), count = BitConverter.ToInt64(index, 8);
            if (first < 0 || count < 0 || first + count > n) return null;
            var files = new List<ImageFile>();
            for (long i = first; i < first + count; i++)
            {
                int e = (int)(i * 24);
                long a = BitConverter.ToInt64(index, e), b = BitConverter.ToInt64(index, e + 8);
                uint nameAt = BitConverter.ToUInt32(index, e + 16);
                int nameLen = BitConverter.ToUInt16(index, e + 20);
                if (index[e + 22] != 1) continue;   // a file, not a folder
                if (poolStart + nameAt + nameLen > isz || a < 0 || b < 0 || a + b > length) return null;
                files.Add(new ImageFile(Encoding.UTF8.GetString(index, (int)(poolStart + nameAt), nameLen), a, b));
            }
            return files;
        }
        catch (Exception) { return null; }
    }

    /// The host's rule for the language version (nvn_mtl.m src_compile): MSL 4.0 only for a source that uses vertex_value, so the
    /// decision is made from the text, and the same text gives the same options here and there.
    public static bool NeedsMsl4(string text) => text.Contains("vertex_value<", StringComparison.Ordinal);

    /// Sixteen hex digits of the SHA-256 of the source bytes: what tells a rebuilt library from the one that was warmed under the same name.
    public static string Digest(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes)).Substring(0, 16).ToLowerInvariant();

    /// The first line of warmed.tsv: the app build and the host's chain. Anything else in its place and the rows are not trusted.
    public static string Header(string appBuild, string chain) => $"# warmed app_build {appBuild} chain {chain}";

    /// The libraries warmed under this header ("name\tdigest" keys), from the file's text; empty when the header is another one.
    public static HashSet<string> ParseWarmed(string text, string header)
    {
        var done = new HashSet<string>(StringComparer.Ordinal);
        if (string.IsNullOrEmpty(text)) return done;
        var lines = text.Split('\n');
        if (lines[0].TrimEnd('\r') != header) return done;
        foreach (var raw in lines.Skip(1))
        {
            var c = raw.TrimEnd('\r').Split('\t');
            if (c.Length >= 3 && c[0].Length > 0 && c[1].Length > 0) done.Add(c[0] + "\t" + c[1]);
        }
        return done;
    }

    /// One row: the library's name, its digest and the milliseconds the compile took (-1: it did not compile, so it is not tried again).
    public static string Row(string name, string digest, long ms) => $"{name}\t{digest}\t{ms}\n";

    /// "Preparing the shaders… 120 of 900", the line the run card and the player screen show.
    public static string StatusText(int done, int total) => $"Preparing the shaders… {done} of {total}";

    // ---- What the game used (used_shaders.tsv) ---------------------------------------------------------------------------------
    // The host already records every shader it had to load in Documents/games/<id>/pso_user.tsv (DM_SAVE is games/<id>/save and the file goes in
    // its parent; nvn_mtl.m pso_user_record): S rows "S\t<key>_b\t<stage>" at a shader's first load, C rows for compute, R and M rows holding
    // the hex of a pso_skey whose first 48 bytes are the vertex and fragment keys (nvn_pso_skey.h). That file is a set (duplicates dropped, in
    // first-use order) with no sessions, so this app keeps its own file beside it: used_shaders.tsv, the keys with a count and the session they
    // were last seen in, ingested from the bytes pso_user.tsv gained since the last ingest. No host patch.

    public const int UsedCap = 30000;   // rows kept: ~1 MB; a 20,000-shader game plays through a fraction of them

    public sealed class UsedRow { public string Key; public int Count; public int Session; public int Pos; }

    private static bool KeyOk(string k)
    {
        if (string.IsNullOrEmpty(k) || k.Length > 24) return false;
        foreach (var c in k) if (!(char.IsAsciiLetterOrDigit(c) || c == '_')) return false;
        return true;
    }

    private static string KeyFromHex(string hex, int byteAt)
    {
        var sb = new StringBuilder();
        for (int i = 0; i < 24; i++)
        {
            int at = (byteAt + i) * 2;
            if (at + 2 > hex.Length) return null;
            if (!byte.TryParse(hex.AsSpan(at, 2), System.Globalization.NumberStyles.AllowHexSpecifier, null, out var b)) return null;
            if (b == 0) break;
            sb.Append((char)b);
        }
        return sb.Length > 0 && KeyOk(sb.ToString()) ? sb.ToString() : null;
    }

    /// The distinct shader keys the rows of a pso_user.tsv name, in first-use order. S and C rows: the key; R and M rows: the vertex and the
    /// fragment key inside the hex (a render pipeline uses libraries a shader row never named when the shader came from the seed list).
    public static List<string> KeysFromPsoRows(string text)
    {
        var keys = new List<string>(); var seen = new HashSet<string>(StringComparer.Ordinal);
        void Add(string k) { if (k != null && KeyOk(k) && seen.Add(k)) keys.Add(k); }
        foreach (var raw in (text ?? "").Split('\n'))
        {
            var c = raw.TrimEnd('\r').Split('\t');
            if (c.Length < 2 || c[0].Length != 1) continue;
            switch (c[0][0])
            {
                case 'S': case 'C': Add(c[1]); break;
                case 'R': Add(KeyFromHex(c[1], 0)); Add(KeyFromHex(c[1], 24)); break;
                case 'M': Add(KeyFromHex(c[1].Length > 0 ? c[1] : c.Length > 2 ? c[2] : "", 0)); if (c.Length > 2) { Add(KeyFromHex(c[2], 0)); Add(KeyFromHex(c[2], 24)); } if (c[1] != "-") Add(c[1]); break;
            }
        }
        return keys;
    }

    public static string UsedHeader(int session, long psoOffset) => $"# used_shaders session {session} pso_offset {psoOffset}";

    /// The rows of a used_shaders.tsv: its session number and the byte offset of pso_user.tsv already ingested.
    public static List<UsedRow> ParseUsed(string text, out int session, out long psoOffset)
    {
        session = 0; psoOffset = 0;
        var rows = new List<UsedRow>();
        if (string.IsNullOrEmpty(text)) return rows;
        foreach (var raw in text.Split('\n'))
        {
            var line = raw.TrimEnd('\r');
            if (line.StartsWith("# used_shaders", StringComparison.Ordinal))
            {
                var w = line.Split(' ');
                for (int i = 0; i + 1 < w.Length; i++)
                {
                    if (w[i] == "session") int.TryParse(w[i + 1], out session);
                    else if (w[i] == "pso_offset") long.TryParse(w[i + 1], out psoOffset);
                }
                continue;
            }
            var c = line.Split('\t');
            if (c.Length >= 3 && KeyOk(c[0]) && int.TryParse(c[1], out var n) && int.TryParse(c[2], out var s) && n > 0)
                rows.Add(new UsedRow { Key = c[0], Count = n, Session = s, Pos = rows.Count });
        }
        return rows;
    }

    /// Most recent session first, then the more often seen, then the earlier first used (the order the game asked for them in).
    public static List<UsedRow> UsedOrder(IEnumerable<UsedRow> rows) =>
        rows.OrderByDescending(r => r.Session).ThenByDescending(r => r.Count).ThenBy(r => r.Pos).ToList();

    /// Ingests what pso_user.tsv gained (`psoBytes`: the file as it is now) into the used file's text; returns the new text (the same text
    /// when nothing is new). A file shorter than the ingested offset was reset or is another game's: read from its start. Only whole lines
    /// are taken (the host appends while this reads); the offset stops after the last one.
    public static string IngestUsed(string usedText, byte[] psoBytes, int cap = UsedCap)
    {
        var rows = ParseUsed(usedText, out var session, out var offset);
        if (offset > psoBytes.Length) offset = 0;
        int end = Array.LastIndexOf(psoBytes, (byte)'\n');
        if (end < offset) return string.IsNullOrEmpty(usedText) ? UsedHeader(session, offset) + "\n" : usedText;
        var chunk = Encoding.UTF8.GetString(psoBytes, (int)offset, (int)(end + 1 - offset));
        var keys = KeysFromPsoRows(chunk);
        if (keys.Count > 0)
        {
            session++;
            var byKey = rows.ToDictionary(r => r.Key, StringComparer.Ordinal);
            foreach (var k in keys)
            {
                if (byKey.TryGetValue(k, out var r)) { r.Count++; r.Session = session; }
                else { r = new UsedRow { Key = k, Count = 1, Session = session, Pos = rows.Count }; rows.Add(r); byKey[k] = r; }
            }
            if (rows.Count > cap)
            {
                // The oldest, least seen go; what stays keeps its first-use order
                var keep = UsedOrder(rows).Take(cap).OrderBy(r => r.Pos).ToList();
                rows = keep;
            }
        }
        var sb = new StringBuilder(UsedHeader(session, end + 1)).Append('\n');
        foreach (var r in rows.OrderBy(r => r.Pos)) sb.Append(r.Key).Append('\t').Append(r.Count).Append('\t').Append(r.Session).Append('\n');
        return sb.ToString();
    }

    /// The file side: Documents/games/<id>/pso_user.tsv into used_shaders.tsv (never throws; the used file is a convenience).
    /// Returns the used keys in warm-up order.
    public static List<string> IngestUsedFile(string documents, string titleId)
    {
        try
        {
            var dir = Path.Combine(documents, "games", titleId.ToUpperInvariant());
            var usedPath = Path.Combine(dir, "used_shaders.tsv");
            var psoPath = Path.Combine(dir, "pso_user.tsv");
            var before = File.Exists(usedPath) ? File.ReadAllText(usedPath) : "";
            var after = before;
            if (File.Exists(psoPath))
            {
                byte[] bytes;
                using (var f = new FileStream(psoPath, FileMode.Open, FileAccess.Read, FileShare.ReadWrite))
                { bytes = new byte[f.Length]; for (int got = 0; got < bytes.Length;) { int r = f.Read(bytes, got, bytes.Length - got); if (r <= 0) { Array.Resize(ref bytes, got); break; } got += r; } }
                after = IngestUsed(before, bytes);
            }
            if (after != before) { Directory.CreateDirectory(dir); var tmp = usedPath + ".part"; File.WriteAllText(tmp, after); File.Move(tmp, usedPath, true); }
            return UsedOrder(ParseUsed(after, out _, out _)).Select(r => r.Key).ToList();
        }
        catch (Exception) { return new List<string>(); }
    }

    /// The warm-up order of the libraries that exist: (a) the used ones (a key's _b library and its _bx and _bf side libraries), (b) the
    /// learned ones, (c) the rest in pack order. `usedCount` is the length of (a), `learnedCount` of (b).
    public static List<string> Order(IReadOnlyList<string> packNames, IReadOnlyList<string> learnedNames, IReadOnlyList<string> usedKeys, out int usedCount, out int learnedCount)
    {
        var exist = new HashSet<string>(packNames, StringComparer.Ordinal); foreach (var n in learnedNames) exist.Add(n);
        var outList = new List<string>(); var seen = new HashSet<string>(StringComparer.Ordinal);
        void Add(string n) { if (exist.Contains(n) && seen.Add(n)) outList.Add(n); }
        foreach (var k in usedKeys)
        {
            Add(k + ".metal");
            if (k.EndsWith("_b", StringComparison.Ordinal)) { Add(k + "x.metal"); Add(k + "f.metal"); }
        }
        usedCount = outList.Count;
        foreach (var n in learnedNames) Add(n);
        learnedCount = outList.Count - usedCount;
        foreach (var n in packNames) Add(n);
        return outList;
    }

    /// "Preparing the shaders… the ones you played: 120 of 140" while the used libraries are being done, then the plain count.
    public static string StatusText(int done, int total, int usedDone, int usedTotal) =>
        usedTotal > 0 && usedDone < usedTotal ? $"Preparing the shaders… the ones you played: {usedDone} of {usedTotal}" : StatusText(done, total);
}

#if !WARM_LOGIC_ONLY
/// The warm-up itself: one run at a time, a couple of low-QoS threads, stoppable at any moment.
internal static class GameWarm
{
    private static readonly object gate = new object();
    private static int generation;            // bumped by Stop and by each Start: a run whose number is not the current one ends
    private static volatile bool active;
    private static string status = "";
    private static string activeGame;

    /// Fired from a worker thread whenever the status moved (the screen hops to the main thread itself).
    public static event Action Changed;

    public static bool Active => active;
    public static string Status => active ? status : "";
    public static string ActiveGame => active ? activeGame : null;

    /// Documents/gameapp.settings.tsv warm_shaders: on unless 0/off/no/false. (No switch on the Settings page yet: Agent B's page.)
    public static bool Enabled
    {
        get
        {
            try
            {
                var p = Path.Combine(GameAppMain.Documents, "gameapp.settings.tsv");
                var text = File.Exists(p) ? File.ReadAllText(p) : "";
                return GameAppUi.AppSetting(text, "warm_shaders", "1").ToLowerInvariant() is not ("0" or "off" or "no" or "false");
            }
            catch (Exception) { return true; }
        }
    }

    /// The same compile the host does (nvn_mtl.m src_compile), with the options it gives. See the file's head before touching this.
    private static string WarmCompile(IMTLDevice device, string text, out string error)
    {
        error = null;
        using var options = new MTLCompileOptions();
        if (OperatingSystem.IsIOSVersionAtLeast(18)) options.MathMode = MTLMathMode.Relaxed;   // the host: o.mathMode = Relaxed (it targets iOS 26)
        options.PreserveInvariance = true;
        if (GameWarmLogic.NeedsMsl4(text) && OperatingSystem.IsIOSVersionAtLeast(26)) options.LanguageVersion = MTLLanguageVersion.v4_0;
        var lib = device.CreateLibrary(text, options, out NSError e);
        if (lib == null) { error = e?.LocalizedDescription ?? "no library"; return null; }
        lib.Dispose();
        return "ok";
    }

    public static void Stop(string why)
    {
        bool was;
        lock (gate) { was = active; generation++; }
        if (was) Log($"shader warm-up: stopped ({why})");
    }

    /// Where the lines go besides the console: the app's log page (set by the view controller).
    public static Action<string> Sink;
    private static void Log(string line) { try { if (Sink != null) Sink(line); else Console.WriteLine(line); } catch (Exception) { } }

    private sealed record Item(string Name, string Path, long Offset, long Size);

    /// Starts preparing `titleId`'s libraries from `packRoot` (Documents/shaders, or the player's bundle copy). Returns at once.
    public static void Start(string titleId, string packRoot)
    {
        if (!Enabled || string.IsNullOrEmpty(titleId) || string.IsNullOrEmpty(packRoot)) return;
        int mine;
        lock (gate) { mine = ++generation; active = true; activeGame = titleId; status = ""; }
        var t = new Thread(() => { try { Run(titleId, packRoot, mine); } catch (Exception error) { Log("shader warm-up: [ORANGE] " + error.Message); } finally { lock (gate) { if (generation == mine) active = false; } Changed?.Invoke(); } })
        { IsBackground = true, Name = "shader-warm-up" };
        t.Start();
    }

    /// The phone may carry on with the libraries nobody asked for: plugged in (or full), or not warm.
    private static bool RestAllowed()
    {
        var thermal = NSProcessInfo.ProcessInfo.ThermalState;
        if (thermal <= NSProcessInfoThermalState.Fair) return true;   // Nominal or Fair
        try
        {
            UIDevice.CurrentDevice.BatteryMonitoringEnabled = true;
            return UIDevice.CurrentDevice.BatteryState is UIDeviceBatteryState.Charging or UIDeviceBatteryState.Full;
        }
        catch (Exception) { return false; }
    }

    private static bool Current(int mine) => Volatile.Read(ref generation) == mine;

    private static void Run(string titleId, string packRoot, int mine)
    {
        NSThread.Current.QualityOfService = NSQualityOfService.Utility;
        var documents = GameAppMain.Documents;
        var chain = GameAppBuild.ShaderChain;
        var pack = Directory.Exists(packRoot)
            ? Directory.EnumerateDirectories(packRoot).FirstOrDefault(d => string.Equals(Path.GetFileName(d), titleId, StringComparison.OrdinalIgnoreCase)) : null;
        var byName = new Dictionary<string, Item>(StringComparer.Ordinal);
        var packNames = new List<string>(); var learnedNames = new List<string>();
        var learnedDir = Path.Combine(documents, "games", titleId.ToUpperInvariant(), "learned_shaders", "b-" + chain);
        // The library's own copy of a name wins over the learned one (the host reads the learned folder last, msl_data)
        if (pack != null)
        {
            var image = Path.Combine(pack, "b-" + chain + ".img");
            var files = File.Exists(image) ? GameWarmLogic.ReadImageIndex(image) : null;
            if (files != null)
                foreach (var f in files)
                    if (f.Name.EndsWith(".metal", StringComparison.Ordinal) && !byName.ContainsKey(f.Name)) { byName[f.Name] = new Item(f.Name, image, f.Offset, f.Size); packNames.Add(f.Name); }
        }
        if (Directory.Exists(learnedDir))
            foreach (var p in Directory.EnumerateFiles(learnedDir, "*.metal"))
                if (!byName.ContainsKey(Path.GetFileName(p))) { byName[Path.GetFileName(p)] = new Item(Path.GetFileName(p), p, 0, -1); learnedNames.Add(Path.GetFileName(p)); }
        // What the player used first (pso_user.tsv, kept across sessions in used_shaders.tsv), then what the game learned while it ran (the
        // library's are 100 to 22,000, most never used), then the rest in pack order
        var usedKeys = GameWarmLogic.IngestUsedFile(documents, titleId);
        var items = GameWarmLogic.Order(packNames, learnedNames, usedKeys, out int usedCount, out int learnedCount).Select(n => byName[n]).ToList();
        int played = usedCount, front = usedCount + learnedCount;   // the first `front` are what a session needs; the rest waits for a charger or a cool phone
        if (items.Count == 0) { Log($"shader warm-up: nothing to compile for {titleId} (no .metal source in its pack: a Mac-made pack holds .metallib)"); return; }

        var gameDir = Path.Combine(documents, "games", titleId.ToUpperInvariant());
        var warmedPath = Path.Combine(gameDir, "warmed.tsv");
        var header = GameWarmLogic.Header(GameAppBuild.Id, chain);
        HashSet<string> warmed;
        try { warmed = GameWarmLogic.ParseWarmed(File.Exists(warmedPath) ? File.ReadAllText(warmedPath) : "", header); }
        catch (Exception) { warmed = new HashSet<string>(); }
        Directory.CreateDirectory(gameDir);
        if (warmed.Count == 0) File.WriteAllText(warmedPath, header + "\n");   // another build's rows are dropped here, with its header
        var device = MTLDevice.SystemDefault;
        if (device == null) { Log("shader warm-up: no Metal device"); return; }

        int total = items.Count, usedDone = 0, done = 0, compiled = 0, failed = 0, skipped = 0, next = -1;
        var digests = new ConcurrentDictionary<string, byte>();   // the same text compiles once (a cut unit's _b/_bx/_bf share one library in the host)
        var rows = new StringBuilder();
        var sw = Stopwatch.StartNew();
        long compileMs = 0;
        void Report()
        {
            status = GameWarmLogic.StatusText(Volatile.Read(ref done), total, Volatile.Read(ref usedDone), played);
            Changed?.Invoke();
        }
        void Flush(bool force)
        {
            string text;
            lock (rows) { if (rows.Length == 0 || (!force && rows.Length < 4096)) return; text = rows.ToString(); rows.Clear(); }
            try { File.AppendAllText(warmedPath, text); } catch (Exception) { }
        }
        Log($"shader warm-up: {total} librar{(total == 1 ? "y" : "ies")} of {titleId} to prepare so the first session finds them in Metal's cache ({played} used in earlier sessions, {learnedCount} learned while it ran; the other {total - front} wait for a charger or a cool phone)");
        Report();

        int workers = Math.Min(2, Math.Max(1, Environment.ProcessorCount / 2));
        var threads = new List<Thread>();
        for (int w = 0; w < workers; w++)
        {
            int id = w;
            var th = new Thread(() =>
            {
                NSThread.Current.QualityOfService = NSQualityOfService.Utility;
                while (Current(mine))
                {
                    // Heat: Fair keeps one worker, Serious and Critical none, until the phone cools (or the run is stopped)
                    var thermal = NSProcessInfo.ProcessInfo.ThermalState;
                    if (thermal >= NSProcessInfoThermalState.Serious || (thermal >= NSProcessInfoThermalState.Fair && id > 0)) { Thread.Sleep(3000); continue; }
                    // Memory: the ceiling iOS grants is shared with a build or a game; stay out of the way below 700 MiB (a simulator answers none)
                    if (GameAppMain.AvailableMiBValue() is long left && left > 0 && left < 700) { Thread.Sleep(3000); continue; }
                    // After what a session needs: the rest only on a charger or while the phone is not warm (thermal <= Fair), to spare the battery
                    if (Volatile.Read(ref next) + 1 >= front && !RestAllowed()) { Thread.Sleep(5000); continue; }
                    int i = Interlocked.Increment(ref next);
                    if (i >= total) return;
                    var item = items[i];
                    if (i < played) Interlocked.Increment(ref usedDone);   // counted when taken: the count the run card shows leads by at most the two in flight
                    using var pool = new NSAutoreleasePool();
                    byte[] bytes;
                    try
                    {
                        if (item.Size >= 0)
                        {
                            bytes = new byte[item.Size];
                            using var f = new FileStream(item.Path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
                            f.Position = item.Offset;
                            for (int got = 0; got < bytes.Length;) { int r = f.Read(bytes, got, bytes.Length - got); if (r <= 0) throw new IOException("short read"); got += r; }
                        }
                        else bytes = File.ReadAllBytes(item.Path);
                    }
                    catch (Exception) { Interlocked.Increment(ref done); continue; }
                    var digest = GameWarmLogic.Digest(bytes);
                    var key = item.Name + "\t" + digest;
                    bool already;
                    lock (warmed) already = warmed.Contains(key);
                    if (already || !digests.TryAdd(digest, 0)) { Interlocked.Increment(ref skipped); Interlocked.Increment(ref done); if (!already) lock (rows) rows.Append(GameWarmLogic.Row(item.Name, digest, 0)); if (i % 20 == 0) Report(); continue; }
                    string text;
                    try { text = new UTF8Encoding(false, true).GetString(bytes); } catch (Exception) { text = null; }
                    long ms = -1;
                    if (text != null)
                    {
                        var t0 = Stopwatch.GetTimestamp();
                        var ok = WarmCompile(device, text, out var error);
                        ms = ok != null ? (long)Stopwatch.GetElapsedTime(t0).TotalMilliseconds : -1;
                        if (ok == null) { if (Interlocked.Increment(ref failed) <= 3) Log($"shader warm-up: {item.Name} did not compile: {error}"); }
                        else { Interlocked.Increment(ref compiled); Interlocked.Add(ref compileMs, ms); }
                    }
                    else Interlocked.Increment(ref failed);
                    lock (rows) rows.Append(GameWarmLogic.Row(item.Name, digest, ms));
                    Flush(false);
                    Interlocked.Increment(ref done);
                    Report();
                }
            }) { IsBackground = true, Name = "shader-warm-" + w };
            threads.Add(th);
            th.Start();
        }
        foreach (var th in threads) th.Join();
        Flush(true);
        int finished = Volatile.Read(ref done);
        if (Current(mine))
            Log($"shader warm-up: {titleId} done, {compiled} compiled in {sw.Elapsed.TotalSeconds:F0} s ({compileMs / 1000.0:F0} s of compile time summed over {workers} thread(s)), {skipped} already warm or shared a source, {failed} did not compile ({finished} of {total})");
        else
            Log($"shader warm-up: {titleId} stopped at {finished} of {total}: {compiled} compiled in {sw.Elapsed.TotalSeconds:F0} s (what is done is kept in warmed.tsv; the next start goes on)");
    }
}

public sealed partial class GameAppViewController
{
    /// After a successful build (Finish), or at a player's first launch: prepare the game's shaders in the background.
    private void StartWarm(string game) => StartWarmTitle(string.IsNullOrEmpty(game) ? null : GameAppUi.BaseTitleIdOf(GameExport.IsPlayer ? GameExport.PlayerDump : game));

    private void StartWarmTitle(string tid)
    {
        if (tid == null || !GameWarm.Enabled) return;
        GameWarm.Sink = line => BeginInvokeOnMainThread(() => Write(line));
        var root = GameExport.IsPlayer ? Path.Combine(GameExport.BundleGame, "shaders") : Path.Combine(GameAppMain.Documents, "shaders");
        GameWarm.Changed -= WarmChanged;
        GameWarm.Changed += WarmChanged;
        GameWarm.Start(tid, root);
    }

    /// A run (a build or a boot) starts: the warm-up gives the phone back.
    private void StopWarm(string why) => GameWarm.Stop(why);

    private void WarmChanged() => BeginInvokeOnMainThread(ShowWarm);

    /// The status on the run card of the game being prepared (after "Built in ..."), or on a player's title screen.
    private void ShowWarm()
    {
        if (GameExport.IsPlayer)
        {
            if (playerStatus == null) return;
            if (GameWarm.Active) { playerStatus.Text = GameWarm.Status; playerStatus.Hidden = false; }
            else RefreshScreen();   // done: the screen says what it says
            return;
        }
        if (runCardView == null || running) return;
        RefreshRunLine();
        if (GameWarm.Active && GameWarm.Status.Length > 0 && !runCardView.Hidden && !failedRun) runLine.Text += "\n" + GameWarm.Status;
    }

    /// At launch: a player prepares its own cache (its bundle identifier is its own: the build's warm-up filled another app's).
    /// A debug launch can name a game with the launch argument "-GAMEAPP_WARM_TITLE <title id>" (the simulator's check: it reaches
    /// NSUserDefaults; an environment variable cannot be set through the device tools).
    private void StartWarmAtLaunch()
    {
        if (GameExport.IsPlayer) { StartWarm(GameExport.PlayerDump); return; }
        var test = NSUserDefaults.StandardUserDefaults.StringForKey("GAMEAPP_WARM_TITLE");
        if (!string.IsNullOrEmpty(test)) StartWarmTitle(test.ToUpperInvariant());
    }
}
#endif
