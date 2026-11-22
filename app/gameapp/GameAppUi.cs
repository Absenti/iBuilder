// gameapp/GameAppUi.cs -- what the screen says, as plain functions of plain values.
//
// No UIKit, no Foundation, no file access: GameAppViewController (GameAppMain.cs) gathers the facts from Documents
// and the Keychain, hands them over as a UiFacts, and draws what comes back. That split is the point of this file:
// the decisions the screen makes -- is the app ready to build, what does the main button do now, how far along is
// the chain -- are testable on a Mac with `gameapp/run_tests.sh`, where the view controller cannot be (it needs the
// iOS SDK). It is compiled into the app next to GameAppMain.cs (gameapp/build_gameapp.sh) and into
// gameapp/tests/GameAppUi.Tests.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Text.RegularExpressions;

/// How a checklist row is drawn: green, orange, red, or neutral.
public enum CheckLevel { Ok, Warn, Missing, Info }

/// The state of Documents/gameapp.boot-only (docs/DECISIONS.md trap 28): nothing pending; ARMED, a request nobody
/// has tried; ATTEMPTED, a boot began from it and never came back.
public enum BootRequestState { None, Armed, Attempted }

/// Whether the framework on disk can be booted without a rebuild. NONE: there is none (or no extraction beside it).
/// FRESH: this build of the app built it (Documents/gameapp.framework names the same build id). STALE: it exists, but
/// another build of the app made it, or it predates the stamp: booting it would run an older host and rewrite.
public enum FrameworkState { None, Fresh, Stale }

/// What gameapp_pipeline.py writes to Documents/gameapp.framework right after it signs a framework.
public sealed record FrameworkStamp(string AppBuild, string Game, string Content = "");

/// What the main button does when tapped.
public enum ActionKind { Busy, Import, Boot, Run, Export }

public sealed record ChecklistRow(string Label, string Detail, CheckLevel Level);

public sealed record PrimaryAction(ActionKind Kind, string Title, bool Enabled);

/// One `Documents/gameapp.stage` line, read: `stage 2/5: rewriting the modules (1/3: main)`. Fraction is where the
/// bar stands when that step STARTS, 0..1.
public sealed record StageProgress(bool Valid, int Step, int Of, string Text, double Fraction);

/// Everything the screen's decisions depend on. `Games` are file names. `IdentityState` is what the shim's
/// gameapp_identity_status returns (1 = a .p12 is held, 0 = none, anything else = unreadable). `AvailableMiB` is
/// os_proc_available_memory in MiB, -1 where there is no such limit (a simulator).
public sealed record UiFacts(bool Running, bool HasKeys, IReadOnlyList<string> Games, int IdentityState,
                             long P12Bytes, long AvailableMiB, BootRequestState BootRequest,
                             FrameworkState Framework = FrameworkState.None, string FrameworkGame = null,
                             IReadOnlyList<string> ShaderPacks = null, IReadOnlyList<string> PendingShaderPacks = null,
                             int ExtendedAddressing = -1, double AddressSpaceFreeGiB = -1, int IncreasedMemory = -1,
                             bool PlayerMode = false, int LoadsFromDocuments = -1, string FrameworkContent = null);

public static class GameAppUi
{
    /// What the device is called in the words the screen says ("this iPad"). The app sets it at launch; the tests run on a Mac and read the default.
    public static string DeviceName = "iPhone";

    /// The first word of Documents/gameapp.boot-only once a boot has begun from it. gameapp_pipeline.py writes it
    /// (BOOT_ONLY_ATTEMPTED); anything else in the file is an armed request.
    public const string BootOnlyAttempted = "attempted";

    /// At or above this ceiling the engine keeps the guest pool in sparse files and the chain boots in its own
    /// process (gameapp_shim.c: as-reserve skipped; gameapp_pipeline.py: boot_floor_mib). Below it the build and the
    /// boot are two launches.
    public const long FullChainCeilingMiB = 5 * 1024;

    // ---- the memory ceiling ---------------------------------------------------------------------------------

    /// os_proc_available_memory() in bytes -> the checklist's MiB, -1 when there is no ceiling to show. A simulator
    /// answers 0 (no per-process limit there; the shim passes it through, and gameapp_pipeline.py reads 0 the same
    /// way), and the shim answers -1 where the call does not exist: neither is a ceiling. Read as a number, the
    /// simulator's 0 drew "0 MiB, the build and the boot take two launches" -- seen on the first run of the real
    /// app, where the preview's stub had answered -1.
    public static long CeilingMiB(long availableBytes) =>
        availableBytes <= 0 ? -1 : availableBytes / (1024 * 1024);

    // ---- the boot-only request ------------------------------------------------------------------------------

    /// `exists` is whether the file is there; `content` what it says, null when it could not be read. An
    /// unreadable request is treated as attempted: the safe reading, which never starts anything by itself.
    public static BootRequestState ClassifyBootRequest(bool exists, string content)
    {
        if (!exists) return BootRequestState.None;
        if (content == null) return BootRequestState.Attempted;
        return content.StartsWith(BootOnlyAttempted, StringComparison.Ordinal)
            ? BootRequestState.Attempted
            : BootRequestState.Armed;
    }

    // ---- the framework on disk ------------------------------------------------------------------------------

    /// The stamp is `key=value` lines (app_build, game, built). Null when there is no stamp or it names no build.
    public static FrameworkStamp ParseFrameworkStamp(string content)
    {
        if (content == null) return null;
        string build = null, game = null, built = null;   // no content= line: a stamp from before builds recorded it
        foreach (var raw in content.Split('\n'))
        {
            var line = raw.Trim();
            var eq = line.IndexOf('=');
            if (eq <= 0) continue;
            var key = line.Substring(0, eq).Trim();
            var value = line.Substring(eq + 1).Trim();
            if (key == "app_build") build = value;
            else if (key == "game") game = value;
            else if (key == "content") built = value;
        }
        return string.IsNullOrEmpty(build) ? null : new FrameworkStamp(build, game ?? "", built);
    }

    /// `onDisk`: the framework and the extraction it boots from are both there. `stampContent`: what
    /// Documents/gameapp.framework says, null when it is missing or unreadable. `currentBuild`: this app's build id.
    /// A framework with no usable stamp is STALE, not fresh: it predates builds being tracked, and the safe reading of
    /// "I cannot tell who built this" is "not this build".
    public static FrameworkState ClassifyFramework(bool onDisk, string stampContent, string currentBuild)
    {
        if (!onDisk) return FrameworkState.None;
        var stamp = ParseFrameworkStamp(stampContent);
        if (stamp == null || string.IsNullOrEmpty(currentBuild)) return FrameworkState.Stale;
        return string.Equals(stamp.AppBuild, currentBuild, StringComparison.Ordinal)
            ? FrameworkState.Fresh
            : FrameworkState.Stale;
    }

    // ---- the checklist --------------------------------------------------------------------------------------

    /// What the user has to know before tapping, in the order they can act on it. Rows that do not apply are not
    /// there (the framework row only exists while a boot is pending).
    public static IReadOnlyList<ChecklistRow> Checklist(UiFacts f)
    {
        var rows = new List<ChecklistRow>();

        rows.Add(f.HasKeys
            ? new ChecklistRow("prod.keys", "in Documents", CheckLevel.Ok)
            : new ChecklistRow("prod.keys", "missing — import it", CheckLevel.Missing));

        var games = f.Games ?? Array.Empty<string>();
        if (games.Count == 0)
            rows.Add(new ChecklistRow("Game", "none — import a .nsp, .xci, .nsz or .nup", CheckLevel.Missing));
        else if (games.Count == 1)
            rows.Add(new ChecklistRow("Game", games[0], CheckLevel.Ok));
        else
            rows.Add(new ChecklistRow("Game", Inv($"{games.Count} games, first: {games[0]}"), CheckLevel.Ok));

        if (f.IdentityState == 1)
            rows.Add(new ChecklistRow("Signing certificate", Inv($"stored in the Keychain ({f.P12Bytes} bytes)"), CheckLevel.Ok));
        else if (f.IdentityState == 0)
            rows.Add(new ChecklistRow("Signing certificate", "none — a device needs it to sign the framework (Certificate)", CheckLevel.Warn));
        else
            rows.Add(new ChecklistRow("Signing certificate", "unreadable", CheckLevel.Warn));

        // The two entitlements, as checks the player can read at a glance (both are fixed at install, not switches).
        rows.Add(RamUnlockRow(f.IncreasedMemory, f.AvailableMiB));
        if (!f.PlayerMode) rows.Add(RunsHereRow(f.LoadsFromDocuments));
        rows.Add(ExtendedAddressingRow(f.ExtendedAddressing, f.AddressSpaceFreeGiB));

        if (f.AvailableMiB < 0)
            rows.Add(new ChecklistRow("Memory ceiling", "no per-process limit here (a simulator)", CheckLevel.Info));
        else if (f.AvailableMiB >= FullChainCeilingMiB)
            rows.Add(new ChecklistRow("Memory ceiling", Inv($"{f.AvailableMiB} MiB — enough to build and boot in one launch"), CheckLevel.Ok));
        else
            rows.Add(new ChecklistRow("Memory ceiling", Inv($"{f.AvailableMiB} MiB — the build and the boot take two launches"), CheckLevel.Warn));

        // The address space (docs/DECISIONS.md trap 44): a property of how the app was signed at install, not a switch.
        rows.Add(AddressSpaceRow(f.ExtendedAddressing, f.AddressSpaceFreeGiB));

        // The framework row says whether a boot is possible without a rebuild, and why not when it is not.
        if (f.Framework == FrameworkState.Stale)
            rows.Add(new ChecklistRow("Framework", "built by an older version of the app (or before builds were tracked) — rebuild it", CheckLevel.Warn));
        else if (f.Framework == FrameworkState.None && f.BootRequest != BootRequestState.None)
            rows.Add(new ChecklistRow("Framework", "a boot is requested but no framework is built here — rebuild it", CheckLevel.Warn));
        else if (f.Framework == FrameworkState.Fresh && f.BootRequest == BootRequestState.Armed)
            rows.Add(new ChecklistRow("Framework", "built and signed — the boot is pending", CheckLevel.Ok));
        else if (f.Framework == FrameworkState.Fresh && f.BootRequest == BootRequestState.Attempted)
            rows.Add(new ChecklistRow("Framework", "built and signed — the last boot never came back", CheckLevel.Warn));
        else if (f.Framework == FrameworkState.Fresh)
            rows.Add(new ChecklistRow("Framework",
                string.IsNullOrEmpty(f.FrameworkGame)
                    ? "built and signed — boot it without rebuilding"
                    : "built from " + f.FrameworkGame + " — boot it without rebuilding", CheckLevel.Ok));

        // The shaders of the game this screen is about (the framework's, else the first dump): a game without its
        // translated shaders runs and is heard while every draw is skipped. The pack is made on a Mac
        // (tools/make_shader_pack.sh) for any game the engine's NSP script or NXPocket has built, and matched by title id.
        if (games.Count > 0)
        {
            var file = f.Framework == FrameworkState.Fresh && !string.IsNullOrEmpty(f.FrameworkGame) ? f.FrameworkGame : games[0];
            var id = BaseTitleIdOf(file);
            if (id == null)
                rows.Add(new ChecklistRow("Shaders", "the file name carries no [title id]: its pack is matched at the boot", CheckLevel.Info));
            else if (f.ShaderPacks != null && f.ShaderPacks.Contains(id))
                rows.Add(new ChecklistRow("Shaders", "pack for " + id + " installed", CheckLevel.Ok));
            else if (f.PendingShaderPacks != null && f.PendingShaderPacks.Contains(id))
                rows.Add(new ChecklistRow("Shaders", "pack for " + id + " imported — installed at the next boot", CheckLevel.Ok));
            else
                rows.Add(new ChecklistRow("Shaders", "not built yet for " + id + $" — the next run translates them on this {DeviceName} "
                                          + "(once per game, about a quarter of an hour)", CheckLevel.Info));
        }

        return rows;
    }

    private static readonly Regex TitleIdInName = new Regex(@"\[([0-9A-Fa-f]{16})\]");

    /// The base game's title id named in a dump's file name (`… [0100DFA027504000][v0].nsp`), which is the id the
    /// profile and the shader pack carry. A base game's id ends in 000, its update's in 800 (same block), an add-on's
    /// (DLC) is in the NEXT block, base + 0x1000 + n. Null when the name has none.
    public static string BaseTitleIdOf(string fileName)
    {
        var id = TitleIdValue(fileName);
        if (id == null) return null;
        var v = id.Value;
        var kind = ContentKindOf(fileName);
        var baseId = kind == ContentKind.AddOn ? (v - 0x1000UL) & ~0xFFFUL : v & ~0xFFFUL;
        return baseId.ToString("X16", CultureInfo.InvariantCulture);
    }

    private static ulong? TitleIdValue(string fileName)
    {
        if (string.IsNullOrEmpty(fileName)) return null;
        var m = TitleIdInName.Match(fileName);
        if (!m.Success) return null;
        return ulong.Parse(m.Groups[1].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    public enum ContentKind { Base, Update, AddOn, Unknown }

    /// What a dump is, from the title id in its name: low 12 bits 000 = a base game, 800 = its update, anything else =
    /// an add-on (DLC). No id in the name: Unknown (treated as a game, the extractor decides).
    public static ContentKind ContentKindOf(string fileName)
    {
        var id = TitleIdValue(fileName);
        if (id == null) return ContentKind.Unknown;
        var low = id.Value & 0xFFFUL;
        return low == 0 ? ContentKind.Base : low == 0x800 ? ContentKind.Update : ContentKind.AddOn;
    }

    private static readonly Regex VersionInName = new Regex(@"\[v(\d+)\]");

    /// The games the library shows: base games and dumps with no id. An update or an add-on is not a game of its own
    /// (extracting an update as a base game fails); the update rides with its base game (UpdateFor).
    public static List<string> PlayableGames(IEnumerable<string> fileNames) =>
        (fileNames ?? Array.Empty<string>()).Where(f => ContentKindOf(f) is ContentKind.Base or ContentKind.Unknown).ToList();

    /// The newest update in the list for this base game (same base id, kind Update, highest [vN]), or null: the extractor
    /// takes it as its update argument and merges it over the base game's RomFS and code.
    public static string UpdateFor(string baseFile, IEnumerable<string> fileNames)
    {
        var id = BaseTitleIdOf(baseFile);
        if (id == null || ContentKindOf(baseFile) != ContentKind.Base) return null;
        return (fileNames ?? Array.Empty<string>())
            .Where(f => ContentKindOf(f) == ContentKind.Update && BaseTitleIdOf(f) == id)
            .OrderByDescending(f => { var m = VersionInName.Match(f); return m.Success && long.TryParse(m.Groups[1].Value, out var v) ? v : 0; })
            .FirstOrDefault();
    }

    /// The title id in a dump's name as 16 hex digits, or null.
    public static string TitleIdHexOf(string fileName) => TitleIdValue(fileName)?.ToString("X16", CultureInfo.InvariantCulture);

    /// The version in a dump's name ([v458752]), 0 when it has none.
    public static long VersionOf(string fileName)
    {
        var m = VersionInName.Match(fileName ?? "");
        return m.Success && long.TryParse(m.Groups[1].Value, out var v) ? v : 0;
    }

    /// The add-ons (DLC) in the list for this base game: same base id, kind AddOn; for one DLC title id given twice,
    /// the highest [vN]. In title-id order (their index order).
    public static List<string> DlcFor(string baseFile, IEnumerable<string> fileNames)
    {
        var id = BaseTitleIdOf(baseFile);
        if (id == null || ContentKindOf(baseFile) != ContentKind.Base) return new List<string>();
        return (fileNames ?? Array.Empty<string>())
            .Where(f => ContentKindOf(f) == ContentKind.AddOn && BaseTitleIdOf(f) == id)
            .GroupBy(f => TitleIdValue(f))
            .Select(g => g.OrderByDescending(VersionOf).First())
            .OrderBy(f => TitleIdValue(f))
            .ToList();
    }

    /// A DLC's add-on content index (the host's aoc/<index>.img): its title id minus the game's AddOnContentBaseId
    /// (the NACP's), or minus base id + 0x1000 when the NACP gives none. Null for a name with no id.
    public static long? AocIndex(string dlcFile, ulong baseTitleId, ulong aocBaseId)
    {
        var id = TitleIdValue(dlcFile);
        if (id == null) return null;
        var start = aocBaseId != 0 ? aocBaseId : baseTitleId + 0x1000;
        return id.Value >= start ? (long) (id.Value - start) : (long?) null;
    }

    /// What a build of this game includes besides the game: its update and DLC file names, the framework stamp's
    /// `content=` (gameapp_pipeline write_framework_stamp). A different value now means the player added (or removed)
    /// an update or a DLC after the build.
    public static string ContentSignature(string baseFile, IEnumerable<string> fileNames)
    {
        var parts = new List<string>();
        var update = UpdateFor(baseFile, fileNames);
        if (update != null) parts.Add("u:" + update);
        parts.AddRange(DlcFor(baseFile, fileNames).Select(d => "d:" + d));
        return string.Join("|", parts);
    }

    /// One line for the game's card: "update v458752 · 2 DLC", or null when it has neither.
    /// A DLC as people know it: the "[DLC ...]" its file name carries ("Danger Deck"), else its display name.
    public static string DlcLabel(string fileName)
    {
        var m = Regex.Match(fileName ?? "", @"\[DLC\s+([^\]]+)\]", RegexOptions.IgnoreCase);
        return m.Success ? m.Groups[1].Value.Trim() : DisplayName(fileName);
    }

    /// An update as people know it: "update 2" for v131072 (the version counts updates in steps of 65536), else "v<n>".
    public static string UpdateLabel(string fileName)
    {
        var n = VersionOf(fileName);
        if (n > 0 && n % 65536 == 0) return "update " + (n / 65536).ToString(CultureInfo.InvariantCulture);
        return "v" + n.ToString(CultureInfo.InvariantCulture);
    }

    /// The updates and DLC among `fileNames` that belong to `baseFile` (the same base title id), in name order: what a
    /// folder scan offers to add to a game.
    public static List<string> RelatedContent(string baseFile, IEnumerable<string> fileNames)
    {
        var id = BaseTitleIdOf(baseFile);
        if (id == null) return new List<string>();
        return (fileNames ?? Enumerable.Empty<string>())
            .Where(n => n != baseFile && BaseTitleIdOf(n) == id && (ContentKindOf(n) == ContentKind.Update || ContentKindOf(n) == ContentKind.AddOn))
            .Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(n => n, StringComparer.OrdinalIgnoreCase).ToList();
    }

    public static string ContentLine(string baseFile, IEnumerable<string> fileNames)
    {
        var parts = new List<string>();
        var update = UpdateFor(baseFile, fileNames);
        if (update != null) parts.Add(UpdateLabel(update));
        int dlc = DlcFor(baseFile, fileNames).Count;
        if (dlc > 0) parts.Add(Inv($"{dlc} DLC"));
        return parts.Count == 0 ? null : string.Join(" · ", parts);
    }

    /// True when the framework on disk is this game's, and its update/DLC differ from what is in Documents now.
    public static bool ContentChangedSinceBuild(string baseFile, IReadOnlyList<string> allFiles, UiFacts f) =>
        f.FrameworkContent != null && f.FrameworkGame == baseFile && f.Framework == FrameworkState.Fresh
        && f.FrameworkContent != ContentSignature(baseFile, allFiles);

    // ---- the main button ------------------------------------------------------------------------------------

    /// The one thing the main button does now. A pending boot outranks a rebuild (a rebuild would take ~2.5
    /// minutes to arrive where the app already is); a missing prod.keys or game turns the button into the import
    /// that fixes it; a missing certificate does not stop a run (the rewrite is worth doing on its own).
    public static PrimaryAction ChooseAction(UiFacts f)
    {
        if (f.Running) return new PrimaryAction(ActionKind.Busy, "Running…", false);

        // A boot is offered whenever a framework THIS build made is on disk, with or without a request: a boot that
        // ran to its time limit consumes its request, and the framework it booted is still good. A request alone is
        // not enough (it may point at a framework an older app built, or at none), and a stale framework is never
        // offered: it would run an older host and rewrite.
        if (f.Framework == FrameworkState.Fresh && CannotRunHere(f))
            return new PrimaryAction(ActionKind.Export, "Export as .ipa", true);
        if (f.Framework == FrameworkState.Fresh)
            return f.BootRequest == BootRequestState.Armed
                ? new PrimaryAction(ActionKind.Boot, "Boot the built framework", true)
                : new PrimaryAction(ActionKind.Boot, "Boot again (no rebuild)", true);

        var hasGame = f.Games != null && f.Games.Count > 0;
        if (!f.HasKeys && !hasGame) return new PrimaryAction(ActionKind.Import, "Import prod.keys and a game", true);
        if (!f.HasKeys) return new PrimaryAction(ActionKind.Import, "Import prod.keys", true);
        if (!hasGame) return new PrimaryAction(ActionKind.Import, "Import a game", true);

        return new PrimaryAction(ActionKind.Run, "Build & run a game", true);
    }

    /// The way out of the boot. While a fresh framework is on disk the main button only boots, and the framework may
    /// be for another game than the one now wanted, or simply not what the user wants to keep, so the screen also
    /// offers the rebuild. Only when a rebuild could actually start: nothing running, and the keys and a game to
    /// start it from.
    public static bool ShowRebuild(UiFacts f) =>
        f.Framework == FrameworkState.Fresh && !f.Running && f.HasKeys && f.Games != null && f.Games.Count > 0;

    // Minutes, not a promise: ~2.5 min of rewrite when the game's shaders are already built, a quarter of an hour or more
    // when they are not (stage 3; ~17 min for the whole chain of Grizzy on an iPhone 15 Pro).
    public const string RebuildTitle = "Rebuild everything instead (a few minutes)";

    // ---- the library: one row per game file ---------------------------------------------------

    private static readonly Regex Bracketed = new Regex(@"\[[^\]]*\]", RegexOptions.Compiled);
    private static readonly Regex Parenthesised = new Regex(@"\([^)]*\)", RegexOptions.Compiled);
    private static readonly Regex Spaces = new Regex(@"\s+", RegexOptions.Compiled);

    /// The name a card shows: the file name without its extension and without the dump's bracketed tags
    /// (`[0100DFA027504000]`, `[v0]`, `[Base]`) and parenthesised size (`(5.48 GB)`). A name that is nothing but tags
    /// keeps its file name, so a card is never blank.
    public static string DisplayName(string fileName)
    {
        if (string.IsNullOrWhiteSpace(fileName)) return "";
        var stem = fileName;
        var dot = stem.LastIndexOf('.');
        if (dot > 0 && stem.Length - dot <= 5) stem = stem.Substring(0, dot);
        var name = Spaces.Replace(Parenthesised.Replace(Bracketed.Replace(stem, " "), " "), " ").Trim();
        return name.Length > 0 ? name : stem.Trim();
    }

    public enum GameStatus { Ready, NeedsSetup }

    /// READY: the framework on disk is one THIS build made, from this very game file: it boots without a rebuild.
    /// Anything else needs the chain. (A framework of another game does not make this one ready.)
    public static GameStatus GameStatusOf(string fileName, UiFacts f) =>
        f.Framework == FrameworkState.Fresh && !string.IsNullOrEmpty(fileName) &&
        string.Equals(f.FrameworkGame, fileName, StringComparison.Ordinal)
            ? GameStatus.Ready
            : GameStatus.NeedsSetup;

    public static string StatusChipText(GameStatus status) => status == GameStatus.Ready ? "Ready" : "Needs setup";

    /// The shader pack of a game: installed, imported and waiting for the next boot, or neither (or no id to look up).
    public enum ShaderState { Unknown, Missing, Pending, Ready }

    public static ShaderState ShadersOf(string fileName, UiFacts f)
    {
        var id = BaseTitleIdOf(fileName);
        if (id == null) return ShaderState.Unknown;
        if (f.ShaderPacks != null && f.ShaderPacks.Contains(id)) return ShaderState.Ready;
        if (f.PendingShaderPacks != null && f.PendingShaderPacks.Contains(id)) return ShaderState.Pending;
        return ShaderState.Missing;
    }

    /// The one-line summary under the selected game's name: what is in place and what is missing.
    public static string GameSummary(string fileName, UiFacts f)
    {
        var parts = new List<string>();
        if (GameStatusOf(fileName, f) == GameStatus.NeedsSetup)
            parts.Add(f.HasKeys || IsNup(fileName) ? "Needs setup" : "Needs prod.keys first");   // a .nup needs no keys
        switch (ShadersOf(fileName, f))
        {
            case ShaderState.Ready: parts.Add("Shaders ready"); break;
            case ShaderState.Pending: parts.Add("Shaders imported"); break;
            case ShaderState.Missing: parts.Add("Shaders built at setup"); break;
        }
        if (f.AvailableMiB >= 0) parts.Add(Inv($"Memory {f.AvailableMiB} MiB"));
        return string.Join(" · ", parts);
    }

    /// What the selected game's primary button does for THIS game. Running is the same busy button as ever; the
    /// game whose framework is on disk boots ("Play"); any other game runs the whole chain, which needs the keys.
    public static PrimaryAction GameAction(string fileName, UiFacts f)
    {
        if (f.Running) return new PrimaryAction(ActionKind.Busy, "Running…", false);
        if (GameStatusOf(fileName, f) == GameStatus.Ready)
            return CannotRunHere(f) ? new PrimaryAction(ActionKind.Export, "Export as .ipa to play", true)
                                    : new PrimaryAction(ActionKind.Boot, "Play", true);
        // An exported game's app builds nothing: its game is in its bundle, or it is not playable at all.
        if (f.PlayerMode) return new PrimaryAction(ActionKind.Busy, "Not playable — export it again", false);
        if (!f.HasKeys && !IsNup(fileName)) return new PrimaryAction(ActionKind.Import, "Import prod.keys", true);
        return new PrimaryAction(ActionKind.Run, "Set up and play", true);
    }

    /// The selected game: the one asked for if it is still there, else the one that is ready, else the first.
    public static string SelectGame(IReadOnlyList<string> games, string preferred, UiFacts f)
    {
        if (games == null || games.Count == 0) return null;
        if (preferred != null && games.Contains(preferred)) return preferred;
        return games.FirstOrDefault(g => GameStatusOf(g, f) == GameStatus.Ready) ?? games[0];
    }

    /// Which importer a banner row opens.
    public enum Importer { Keys, Certificate }

    public sealed record Prerequisite(Importer Importer, ChecklistRow Row);

    /// What is missing before anything can run, for the banner above the grid: only prod.keys and the signing
    /// certificate (a missing game is the grid's own empty state). Rows at level Missing or Warn only.
    public static IReadOnlyList<Prerequisite> Prerequisites(UiFacts f)
    {
        var list = new List<Prerequisite>();
        if (f.PlayerMode) return list;   // an exported game needs neither prod.keys nor a certificate: it is built
        var rows = Checklist(f);
        foreach (var row in rows)
        {
            if (row.Level != CheckLevel.Missing && row.Level != CheckLevel.Warn) continue;
            if (row.Label == "prod.keys") list.Add(new Prerequisite(Importer.Keys, row));
            else if (row.Label == "Signing certificate") list.Add(new Prerequisite(Importer.Certificate, row));
        }
        return list;
    }

    // ---- the app's own settings: Documents/gameapp.settings.tsv (the shim and the pipeline read it) ----------------

    /// A key's value in the file's text (KEY<TAB>value lines, `#` comments), or `fallback`. The last line wins.
    public static string AppSetting(string text, string key, string fallback)
    {
        var value = fallback;
        foreach (var raw in (text ?? "").Split('\n'))
        {
            var line = raw.TrimEnd('\r');
            var tab = line.IndexOf('\t');
            if (tab <= 0 || line.StartsWith("#", StringComparison.Ordinal)) continue;
            if (line.Substring(0, tab).Trim() == key) value = line.Substring(tab + 1).Split('#')[0].Trim();
        }
        return value;
    }

    /// The file's text with `key` set to `value`, every other line kept as it was.
    public static string WithAppSetting(string text, string key, string value)
    {
        var lines = (text ?? "").Split('\n').Select(l => l.TrimEnd('\r')).Where(l => l.Length > 0).ToList();
        lines.RemoveAll(l => { var t = l.IndexOf('\t'); return t > 0 && !l.StartsWith("#", StringComparison.Ordinal) && l.Substring(0, t).Trim() == key; });
        lines.Add(key + "\t" + value);
        return string.Join("\n", lines) + "\n";
    }

    /// "Parallel shader preparation": on unless the file says 0 (the free-threaded runtime is slower with it off).
    public static bool ParallelShadersOn(string settingsText) =>
        AppSetting(settingsText, "parallel_shaders", "1").ToLowerInvariant() is not ("0" or "off" or "no" or "false");

    /// "Low performance (cooler, slower)": off unless the settings file says 1 (the pipeline runs at most 2 parallel jobs).
    public static bool LowPerformanceOn(string settingsText) =>
        AppSetting(settingsText, "low_performance", "0").ToLowerInvariant() is "1" or "on" or "yes" or "true";

    // ---- deleting a game ---------------------------------------------------------------------------------------------

    /// What deleting a game removes, as paths relative to Documents: its dump and the update / add-on dumps of the same
    /// game, the NSPs rebuilt from .nsz files, its shader library and any pending pack, its derived profile, and its
    /// folder under games/ (settings, settings list, icon, rewrite cache, temp) -- with games/<id>/save kept unless
    /// `keepSaves` is false. When the build on disk is this game's (`builtGame` the framework stamp's game, or
    /// `builtTitleId` profile.tsv's title id), the extraction (the RomFS, the largest file), the framework and the
    /// boot files go too. Paths may not exist; the caller skips those.
    public sealed record DeletionPlan(IReadOnlyList<string> Files, IReadOnlyList<string> Folders, bool BuildToo);

    public static DeletionPlan PlanDeletion(string fileName, IEnumerable<string> allDumps, string builtGame, string builtTitleId, bool keepSaves)
    {
        var files = new List<string>();
        var folders = new List<string>();
        if (string.IsNullOrEmpty(fileName)) return new DeletionPlan(files, folders, false);
        var id = BaseTitleIdOf(fileName);
        var dumps = new List<string> { fileName };
        if (id != null)
            dumps.AddRange((allDumps ?? Array.Empty<string>()).Where(f => f != fileName && BaseTitleIdOf(f) == id
                && ContentKindOf(f) is ContentKind.Update or ContentKind.AddOn));
        foreach (var d in dumps)
        {
            files.Add(d);
            if (d.EndsWith(".nsz", StringComparison.OrdinalIgnoreCase))
                files.Add(".nsz/" + d.Substring(0, d.Length - 4) + ".nsp");
        }
        if (id != null)
        {
            folders.Add("shaders/" + id);
            folders.Add("shaders/" + id + ".work");
            folders.Add("shaders/" + id + ".part");
            files.Add("shaders/" + id + ".none");
            files.Add(id + ".shaders.tar");
            folders.Add("profile-derived/" + id);
            if (keepSaves)
            {
                foreach (var f in new[] { "settings.tsv", "launcher.json", "icon.jpg", "icon.none", "pso_user.tsv" })
                    files.Add("games/" + id + "/" + f);
                folders.Add("games/" + id + "/rewrite-cache");
                folders.Add("games/" + id + "/temp");
            }
            else folders.Add("games/" + id);
        }
        var buildToo = string.Equals(builtGame, fileName, StringComparison.Ordinal)
                       || (id != null && string.Equals(builtTitleId, id, StringComparison.OrdinalIgnoreCase));
        if (buildToo)
        {
            folders.Add("extract");
            folders.Add("DmGameProbe.framework");
            files.AddRange(new[] { "gameapp.framework", "gameapp.boot-only", "pipeline.boot", "profile.tsv" });
        }
        return new DeletionPlan(files, folders, buildToo);
    }

    /// "1.4 GB" / "350 MB": the space a deletion frees, for the confirmation.
    public static string SizeText(long bytes) =>
        bytes >= 1L << 30 ? Inv($"{bytes / 1073741824.0:0.0} GB")
        : bytes >= 1L << 20 || bytes <= 0 ? Inv($"{Math.Max(0, bytes) / 1048576.0:0} MB")
        : Inv($"{Math.Max(1, bytes / 1024)} KB");   // a save is often a few KB: "0 MB" read as empty

    // ---- the game's icon ---------------------------------------------------------------------------------------------

    /// `games/<base title id>/icon.jpg` relative to Documents (the control data's 256x256 JPEG), or null when the file
    /// name carries no title id. An update or an add-on shares its base game's icon.
    public static string IconRelativePath(string fileName)
    {
        var id = BaseTitleIdOf(fileName);
        return id == null ? null : "games/" + id + "/icon.jpg";
    }

    /// `games/<id>/icon.none`: the dump was read and gave no icon (or could not be read with these keys); tried again
    /// only when prod.keys is newer than the marker.
    public static string NoIconRelativePath(string fileName)
    {
        var id = BaseTitleIdOf(fileName);
        return id == null ? null : "games/" + id + "/icon.none";
    }

    /// A Switch 2 game's package (.nup): not encrypted, read by the chain itself (no prod.keys, no extractor).
    public static bool IsNup(string fileName) => (fileName ?? "").EndsWith(".nup", StringComparison.OrdinalIgnoreCase);

    // ---- a built game exported as its own .ipa (GameAppExport.cs) ---------------------------------------------------

    /// The .ipa's and the .app's name: the game's name with only letters, digits, spaces, '-' and '_' (an .app name
    /// with '/' or ':' breaks the archive; the installers show CFBundleDisplayName anyway). Never empty.
    public static string ExportFileName(string displayName)
    {
        var chars = (displayName ?? "").Where(c => char.IsLetterOrDigit(c) || c == ' ' || c == '-' || c == '_').ToArray();
        var name = Regex.Replace(new string(chars), @"\s+", " ").Trim();
        if (name.Length > 60) name = name.Substring(0, 60).TrimEnd();
        return name.Length == 0 ? "Game" : name;
    }

    /// The export's bundle identifier: GameApp's own, plus the title id in lower case, so it installs beside GameApp
    /// and beside the other exported games. An installer may still rewrite it (SideStore appends its team).
    public static string ExportBundleId(string baseId, string titleId) =>
        (string.IsNullOrEmpty(baseId) ? "dev.nxpocket.gameapp" : baseId) + ".g" + (titleId ?? "").ToLowerInvariant();

    public static string ExportManifestText(string titleId, string dump, string name, string appBuild, string exported) =>
        Inv($"title_id\t{titleId}\ndump\t{dump}\nname\t{name}\napp_build\t{appBuild}\nexported\t{exported}\n");

    // ---- a slim export: the game's files beside the .ipa, not in it -----------------------------------------------
    //
    // A 15.5 GB .ipa (South Park) is more than an installer can repack: FlareStore signed it, then died packaging it.
    // The slim .ipa holds the code, the framework and the shaders; the game's RomFS stays in iBuilder as
    // Exports/<Name> [<title id>].romfs (a clone of extract/RomFS.img: no space taken), and the installed app is shown
    // that file once (Files › iBuilder › Exports), checks it is its game's and keeps a bookmark to read it in place.

    public const string RomfsExtension = ".romfs";

    /// The game files' name beside the .ipa: the .ipa's name and the title id, so the right one is plain to see.
    public static string RomfsFileName(string displayName, string titleId) =>
        $"{ExportFileName(displayName)} [{(titleId ?? "").ToUpperInvariant()}]{RomfsExtension}";

    /// Above this many bytes of game files a Light IPA is the better export: an installer repacks the whole .ipa and
    /// a 15.5 GB one was more than FlareStore could do.
    public const long FullIpaComfortBytes = 4L << 30;

    /// True when the Light IPA is the preferred export for game files this large (more than 4 GB).
    public static bool LightIpaPreferred(long romfsBytes) => romfsBytes > FullIpaComfortBytes;

    /// The game files' place in Exports: a folder named after the game, so the Files app shows one folder per game.
    public static string RomfsRelativePath(string displayName, string titleId) =>
        ExportFileName(displayName) + "/" + RomfsFileName(displayName, titleId);

    /// The game files of the exported app `ipaName` (without ".ipa"): in its folder under Exports, else loose in
    /// Exports (where earlier builds put them). Null when there are none.
    public static string FindRomfs(string exportsDir, string ipaName)
    {
        foreach (var dir in new[] { Path.Combine(exportsDir, ipaName), exportsDir })
        {
            try
            {
                if (!Directory.Exists(dir)) continue;
                var found = Directory.GetFiles(dir, ipaName + " [*]" + RomfsExtension).OrderBy(f => f, StringComparer.Ordinal).FirstOrDefault();
                if (found != null) return found;
            }
            catch (Exception) { }
        }
        return null;
    }

    /// Every .romfs under Exports: loose ones and those in the game folders.
    public static List<string> AllRomfs(string exportsDir)
    {
        var all = new List<string>();
        try
        {
            if (!Directory.Exists(exportsDir)) return all;
            all.AddRange(Directory.GetFiles(exportsDir, "*" + RomfsExtension));
            foreach (var d in Directory.GetDirectories(exportsDir)) all.AddRange(Directory.GetFiles(d, "*" + RomfsExtension));
        }
        catch (Exception) { }
        return all;
    }

    /// Removes the game folder a deleted .romfs leaves empty (never Exports itself). True when one went.
    public static bool RemoveEmptyGameFolder(string exportsDir, string deletedFile)
    {
        try
        {
            var dir = Path.GetDirectoryName(deletedFile);
            if (dir == null || Path.GetFullPath(dir).TrimEnd('/') == Path.GetFullPath(exportsDir).TrimEnd('/')) return false;
            if (!string.Equals(Path.GetDirectoryName(Path.GetFullPath(dir)), Path.GetFullPath(exportsDir).TrimEnd('/'), StringComparison.Ordinal)) return false;
            if (!Directory.Exists(dir) || Directory.EnumerateFileSystemEntries(dir).Any()) return false;
            Directory.Delete(dir);
            return true;
        }
        catch (Exception) { return false; }
    }

    /// The manifest's lines for a slim export: which file to ask for, its size and its check.
    public static string RomfsManifestLines(string fileName, long size, string check) =>
        Inv($"romfs_file\t{fileName}\nromfs_size\t{size}\nromfs_check\t{check}\n");

    /// A quick fingerprint of game files: SHA-256 over the size, the first and the last 4 MiB (a 15 GB file is not
    /// read whole to say "yes, that is South Park's").
    public static string RomfsCheck(Stream stream)
    {
        const int Part = 4 << 20;
        long size = stream.Length;
        using var sha = System.Security.Cryptography.SHA256.Create();
        var head = BitConverter.GetBytes(size);
        sha.TransformBlock(head, 0, head.Length, null, 0);
        var buffer = new byte[Part];
        void Take(long at)
        {
            stream.Seek(at, SeekOrigin.Begin);
            int want = (int) Math.Min(Part, size - at), got = 0, n;
            while (got < want && (n = stream.Read(buffer, got, want - got)) > 0) got += n;
            sha.TransformBlock(buffer, 0, got, null, 0);
        }
        Take(0);
        if (size > Part) Take(Math.Max(Part, size - Part));
        sha.TransformFinalBlock(Array.Empty<byte>(), 0, 0);
        return Convert.ToHexString(sha.Hash).ToLowerInvariant().Substring(0, 32);
    }

    /// Whether a picked file is the game files this export asks for; null when it is, else what to tell the player.
    public static string RomfsMismatch(IReadOnlyDictionary<string, string> manifest, string pickedName, long size, Func<string> check)
    {
        if (manifest == null || !manifest.TryGetValue("romfs_file", out var want)) return null;
        long wantSize = manifest.TryGetValue("romfs_size", out var s) && long.TryParse(s, out var n) ? n : -1;
        var game = manifest.TryGetValue("name", out var g) ? g : "this game";
        if (wantSize >= 0 && size != wantSize)
            return $"{pickedName} is not {game}'s game files ({SizeText(size)}; they are {SizeText(wantSize)}). Pick {want}.";
        if (manifest.TryGetValue("romfs_check", out var c) && !string.IsNullOrEmpty(c) && check() != c)
            return $"{pickedName} has the size of {game}'s game files but not their content (another build of the game?). Pick {want}, exported with this app.";
        return null;
    }

    public static Dictionary<string, string> ParseExportManifest(string text)
    {
        var d = new Dictionary<string, string>();
        foreach (var raw in (text ?? "").Split('\n'))
        {
            var line = raw.TrimEnd('\r');
            var tab = line.IndexOf('\t');
            if (tab > 0) d[line.Substring(0, tab)] = line.Substring(tab + 1);
        }
        return d;
    }

    // ---- the signing certificate's file ---------------------------------------------------------------------------------

    /// Why a picked file cannot be the signing certificate, or null. The picker lists every file (a .p12 a file provider
    /// does not type as com.rsa.pkcs-12 was greyed out), so the name is checked here: .p12 or .pfx, any case.
    public static string CertificateFileProblem(string fileName)
    {
        var ext = Path.GetExtension(fileName ?? "").ToLowerInvariant();
        if (ext == ".p12" || ext == ".pfx") return null;
        if (ext == ".mobileprovision")
            return Inv($"{fileName} is the provisioning profile, not the certificate: choose the .p12 that came with it (the profile is not needed here).");
        if (ext == ".zip")
            return Inv($"{fileName} is an archive: open it in the Files app first, then choose the .p12 inside.");
        return Inv($"{fileName} is not a .p12 certificate: choose the .p12 (or .pfx) file of your signing certificate.");
    }

    /// A PKCS#12 file is one DER SEQUENCE: its first byte is 0x30.
    public static bool IsPkcs12(byte[] bytes) => bytes != null && bytes.Length >= 4 && bytes[0] == 0x30;

    // ---- where a built game can run ------------------------------------------------------------------------------------

    /// A distribution signature (no get-task-allow: FlareStore's certificate) does not let the app map code it signed
    /// itself in Documents ("file system sandbox blocked mmap()" at the dlopen, 2026-11-18): the built game runs only from
    /// an app's own bundle, i.e. exported as an .ipa. An exported game's own app is never in that case (its game is in
    /// its bundle). `LoadsFromDocuments`: 1 yes, 0 no (the signature says so, or a dlopen was refused), -1 unknown.
    public static bool CannotRunHere(UiFacts f) => !f.PlayerMode && f.LoadsFromDocuments == 0;

    public static ChecklistRow RunsHereRow(int loadsFromDocuments)
    {
        if (loadsFromDocuments == 1)
            return new ChecklistRow("Play inside iBuilder", "yes — a development certificate (get-task-allow): a built game runs here", CheckLevel.Ok);
        if (loadsFromDocuments == 0)
            return new ChecklistRow("Play inside iBuilder", "no — a distribution certificate (no get-task-allow) only runs code inside an app: build here, then Export as .ipa and install the game as its own app", CheckLevel.Info);
        return new ChecklistRow("Play inside iBuilder", "unknown (the app's signature could not be read)", CheckLevel.Info);
    }

    // ---- the two entitlements, as yes / no checks ---------------------------------------------------------------------

    /// Above this ceiling iOS has raised the limit: an iPhone 15 Pro (8 GB) gave 3364 MiB without the entitlement and
    /// 6132 MiB with it (the checklist's tests carry both).
    public const long RaisedCeilingMiB = 4096;

    /// "RAM unlocked": the increased-memory-limit entitlement in this app's own signature (1/0, -1 unreadable) and
    /// whether the ceiling shows iOS honoured it (`availableMiB` -1 where there is no limit: a simulator).
    public static ChecklistRow RamUnlockRow(int entitled, long availableMiB)
    {
        if (entitled == 1 && availableMiB < 0)
            return new ChecklistRow("RAM unlocked", "yes — in the signature (a simulator has no limit to raise)", CheckLevel.Ok);
        if (entitled == 1 && availableMiB >= RaisedCeilingMiB)
            return new ChecklistRow("RAM unlocked", Inv($"yes — iOS raised the limit ({availableMiB} MiB available)"), CheckLevel.Ok);
        if (entitled == 1)
            return new ChecklistRow("RAM unlocked", Inv($"in the signature, but iOS kept the normal limit ({availableMiB} MiB available) — the certificate's profile may not allow it"), CheckLevel.Warn);
        if (entitled == 0)
            return new ChecklistRow("RAM unlocked", "no — this install has no increased-memory-limit: install the build with RAM, signed by a certificate that allows it", CheckLevel.Warn);
        return new ChecklistRow("RAM unlocked", "unknown (the app's signature could not be read)", CheckLevel.Info);
    }

    /// "Extended addressing": the extended-virtual-addressing entitlement in this app's own signature (1/0, -1 unknown).
    public static ChecklistRow ExtendedAddressingRow(int extended, double freeGiB)
    {
        if (extended == 1)
            return new ChecklistRow("Extended addressing", "yes — active (larger address space)", CheckLevel.Ok);
        if (extended == 0 && freeGiB >= 16)
            return new ChecklistRow("Extended addressing", "no — not active; iOS still leaves room for most games", CheckLevel.Info);
        if (extended == 0)
            return new ChecklistRow("Extended addressing", "no — not active; a game that reserves a large block may not start", CheckLevel.Warn);
        return new ChecklistRow("Extended addressing", "unknown (the app's signature could not be read)", CheckLevel.Info);
    }

    // ---- the address space ------------------------------------------------------------------------------------------

    /// Extended Virtual Addressing cannot be turned on from inside the app: the installer's signature carries it or not.
    /// The row says which, how much is free, and what a game that reserves a large block (Ocarina of Time: 12.5 GB)
    /// needs. `extended` 1/0, -1 unknown; `freeGiB` -1 unknown.
    public static ChecklistRow AddressSpaceRow(int extended, double freeGiB)
    {
        var free = freeGiB >= 0 ? Inv($" ({freeGiB:0.#} GB free)") : "";
        if (extended == 1)
            return new ChecklistRow("Address space", "extended" + free + " — games that reserve a large memory block can start", CheckLevel.Ok);
        if (extended == 0 && freeGiB >= 16)   // iOS 27 leaves ~64 GB in one hole even without the entitlement (logs 10)
            return new ChecklistRow("Address space", "standard" + free + " — room for games that reserve a large memory block", CheckLevel.Ok);
        if (extended == 0)
            return new ChecklistRow("Address space", "standard" + free + " — most games fit; one that reserves more (Ocarina of Time: 12.5 GB) needs the extended-addressing build, signed with a paid certificate", CheckLevel.Info);
        return new ChecklistRow("Address space", "unknown", CheckLevel.Info);
    }

    // ---- the per-game settings: Documents/games/<base title id>/settings.tsv -------------------------------

    /// `Mode` auto|handheld|docked, `Fps` auto|30|60, `Picture` fit|fill, `ShowFps` 1|0. The pipeline maps them to
    /// the engine's environment; a missing file or key reads as the defaults.
    public sealed record GameSettings(string Mode = "auto", string Fps = "auto", string Picture = "fit", bool ShowFps = true)
    {
        public static readonly GameSettings Defaults = new GameSettings();
        public static GameSettings Parse(string text) => GameAppUi.ParseSettings(text);
        public static string Serialize(GameSettings settings) => GameAppUi.SerializeSettings(settings);
        /// The four legacy fields as the engine's DM_* values say them (the one source of truth once a launcher.json
        /// exists): DM_OPMODE 0|1, DM_FPS_TARGET 30|60, DM_PICTURE_FIT fit|fill, DM_SHOW_FPS 1|0. Missing or unknown is the default.
        public static GameSettings FromValues(IReadOnlyDictionary<string, string> v)
        {
            var s = Defaults;
            if (v == null) return s;
            if (v.TryGetValue("DM_OPMODE", out var m)) { if (m == "0") s = s with { Mode = "handheld" }; else if (m == "1") s = s with { Mode = "docked" }; }
            if (v.TryGetValue("DM_FPS_TARGET", out var f) && (f == "30" || f == "60")) s = s with { Fps = f };
            if (v.TryGetValue("DM_PICTURE_FIT", out var p) && (p == "fit" || p == "fill")) s = s with { Picture = p };
            if (v.TryGetValue("DM_SHOW_FPS", out var h) && (h == "1" || h == "0")) s = s with { ShowFps = h == "1" };
            return s;
        }
    }

    private static readonly string[] Modes = { "auto", "handheld", "docked" };
    private static readonly string[] FpsValues = { "auto", "30", "60" };
    private static readonly string[] Pictures = { "fit", "fill" };

    /// Lines `KEY<TAB>VALUE`, read through ParseSettingValues (so a DM_* file shows its four legacy fields too).
    /// Unknown keys, unknown values, blank and `#` lines are ignored (the key keeps its default); a later line wins.
    public static GameSettings ParseSettings(string text) => GameSettings.FromValues(ParseSettingValues(text));

    /// The file's text: every key, one line each, `\n`-terminated. Values outside the allowed sets are written as
    /// their default, so a file this writes always parses back to the same settings.
    public static string SerializeSettings(GameSettings s)
    {
        var mode = Modes.Contains(s.Mode) ? s.Mode : "auto";
        var fps = FpsValues.Contains(s.Fps) ? s.Fps : "auto";
        var picture = Pictures.Contains(s.Picture) ? s.Picture : "fit";
        return "mode\t" + mode + "\nfps\t" + fps + "\npicture\t" + picture + "\nshow_fps\t" + (s.ShowFps ? "1" : "0") + "\n";
    }

    /// `games/<base title id>/settings.tsv`, relative to Documents; null when the file name carries no title id (the
    /// settings are then disabled: there is no folder to keep them in).
    public static string SettingsRelativePath(string fileName)
    {
        var id = BaseTitleIdOf(fileName);
        return id == null ? null : "games/" + id + "/settings.tsv";
    }

    // ---- the engine launcher's own settings list: Documents/games/<id>/launcher.json ------------------------

    /// One row of launcher.json (docs/launcher.schema.json of the engine). Kind is choice|toggle|range.
    public sealed record LauncherRow(string Env, string Kind, string Section, string Label, string Note, string Default,
                                     bool Basic, string NeedsVar, string NeedsValue,
                                     IReadOnlyList<(string Value, string Label)> Options, string On, string Off,
                                     double Min, double Max, double Step, LauncherBase Base);

    /// Render scale only: the 3D views' size the scale multiplies.
    public sealed record LauncherBase(int DockedW, int DockedH, int HandheldW, int HandheldH);

    private static readonly Regex EnvName = new Regex("^DM_[A-Z0-9_]+$");
    private static readonly IReadOnlyList<(string Value, string Label)> NoOptions = new (string, string)[0];

    private static string JsonText(JsonElement e) => e.ValueKind switch
    {
        JsonValueKind.String => e.GetString(),
        JsonValueKind.Number => e.GetRawText(),
        JsonValueKind.True => "true",
        JsonValueKind.False => "false",
        _ => null,
    };

    private static string Prop(JsonElement o, string name) =>
        o.TryGetProperty(name, out var e) ? JsonText(e) : null;

    private static bool Num(JsonElement o, string name, out double v)
    {
        v = 0;
        return o.TryGetProperty(name, out var e) && e.ValueKind == JsonValueKind.Number && e.TryGetDouble(out v) &&
               !double.IsNaN(v) && !double.IsInfinity(v);
    }

    private static bool Size(JsonElement o, string name, out int w, out int h)
    {
        w = h = 0;
        if (!o.TryGetProperty(name, out var a) || a.ValueKind != JsonValueKind.Array || a.GetArrayLength() < 2) return false;
        return a[0].ValueKind == JsonValueKind.Number && a[0].TryGetInt32(out w) &&
               a[1].ValueKind == JsonValueKind.Number && a[1].TryGetInt32(out h) && w > 0 && h > 0;
    }

    /// The rows of a launcher.json, in file order. Tolerant: unknown fields are ignored, a bad row (no env, unknown kind,
    /// a choice without options, a range without min/max/step, a toggle without on/off) is skipped, and text that is
    /// not a launcher.json at all is no rows. Never throws.
    public static IReadOnlyList<LauncherRow> ParseLauncher(string json)
    {
        var rows = new List<LauncherRow>();
        if (string.IsNullOrWhiteSpace(json)) return rows;
        try
        {
            using var doc = JsonDocument.Parse(json, new JsonDocumentOptions { AllowTrailingCommas = true, CommentHandling = JsonCommentHandling.Skip });
            if (doc.RootElement.ValueKind != JsonValueKind.Object ||
                !doc.RootElement.TryGetProperty("settings", out var list) || list.ValueKind != JsonValueKind.Array) return rows;
            var seen = new HashSet<string>();
            foreach (var o in list.EnumerateArray())
            {
                try
                {
                    var row = ParseLauncherRow(o);
                    if (row != null && seen.Add(row.Env)) rows.Add(row);
                }
                catch (Exception) { /* a bad row is skipped */ }
            }
        }
        catch (Exception) { rows.Clear(); }
        return rows;
    }

    private static LauncherRow ParseLauncherRow(JsonElement o)
    {
        if (o.ValueKind != JsonValueKind.Object) return null;
        var env = Prop(o, "env");
        var kind = Prop(o, "kind");
        if (env == null || !EnvName.IsMatch(env) || kind == null) return null;
        var label = Prop(o, "label");
        var section = Prop(o, "section");
        var row = new LauncherRow(env, kind, string.IsNullOrWhiteSpace(section) ? "Game" : section,
                                  string.IsNullOrWhiteSpace(label) ? env : label, Prop(o, "note") ?? "", Prop(o, "default") ?? "",
                                  Prop(o, "level") == "basic", null, null, NoOptions, null, null, 0, 0, 0, null);
        var needs = Prop(o, "needs");
        if (needs != null)
        {
            var eq = needs.IndexOf('=');
            if (eq > 0 && eq < needs.Length - 1) row = row with { NeedsVar = needs.Substring(0, eq), NeedsValue = needs.Substring(eq + 1) };
        }
        switch (kind)
        {
            case "choice":
            {
                if (!o.TryGetProperty("options", out var opts) || opts.ValueKind != JsonValueKind.Array) return null;
                var list = new List<(string, string)>();
                foreach (var p in opts.EnumerateArray())
                {
                    if (p.ValueKind != JsonValueKind.Array || p.GetArrayLength() < 1) continue;
                    var v = JsonText(p[0]);
                    if (v == null) continue;
                    list.Add((v, p.GetArrayLength() > 1 ? JsonText(p[1]) ?? v : v));
                }
                if (list.Count == 0) return null;
                if (row.Default.Length == 0) row = row with { Default = list[0].Item1 };
                return row with { Options = list };
            }
            case "toggle":
            {
                var on = Prop(o, "on");
                var off = Prop(o, "off");
                if (on == null || off == null) return null;
                return row with { On = on, Off = off };
            }
            case "range":
            {
                if (!Num(o, "min", out var min) || !Num(o, "max", out var max) || !Num(o, "step", out var step) || step <= 0 || max <= min) return null;
                row = row with { Min = min, Max = max, Step = step };
                if (row.Default.Length == 0) row = row with { Default = FormatRange(min, step) };
                if (o.TryGetProperty("base", out var b) && b.ValueKind == JsonValueKind.Object &&
                    b.TryGetProperty("docked", out _) && b.TryGetProperty("handheld", out _) &&
                    Size(b, "docked", out var dw, out var dh) && Size(b, "handheld", out var hw, out var hh))
                    row = row with { Base = new LauncherBase(dw, dh, hw, hh) };
                return row;
            }
            default: return null;
        }
    }

    /// `needs` applied: a row with `needs VAR=value` is visible while VAR's current value (the saved one, else the
    /// default of the row that sets VAR) equals value; a VAR the list does not have leaves the row visible.
    public static IReadOnlyList<LauncherRow> VisibleRows(IReadOnlyList<LauncherRow> rows, IReadOnlyDictionary<string, string> values)
    {
        if (rows == null) return new LauncherRow[0];
        return rows.Where(r => IsVisible(rows, r, values)).ToList();
    }

    public static bool IsVisible(IReadOnlyList<LauncherRow> rows, LauncherRow row, IReadOnlyDictionary<string, string> values)
    {
        if (row.NeedsVar == null) return true;
        var parent = rows.FirstOrDefault(r => r.Env == row.NeedsVar);
        if (parent == null) return true;
        return ValueOf(parent, values) == row.NeedsValue;
    }

    /// The current value of a row: the saved one, else its default.
    public static string ValueOf(LauncherRow row, IReadOnlyDictionary<string, string> values) =>
        values != null && values.TryGetValue(row.Env, out var v) ? v : row.Default;

    /// The rows for this screen: a phone whose display tops out below 120 Hz loses the "120" frame rate.
    public static IReadOnlyList<LauncherRow> FilterForScreen(IReadOnlyList<LauncherRow> rows, int maxFps)
    {
        if (rows == null) return new LauncherRow[0];
        if (maxFps >= 120) return rows;
        return rows.Select(r =>
        {
            if (r.Env != "DM_FPS_TARGET" || r.Kind != "choice" || !r.Options.Any(o => o.Value == "120")) return r;
            var kept = r.Options.Where(o => o.Value != "120").ToList();
            return kept.Count == 0 ? r : r with { Options = kept };
        }).ToList();
    }

    /// Sections in first-appearance order; inside one, the basic rows first, each sub-setting right under its row.
    public static IReadOnlyList<(string Section, IReadOnlyList<LauncherRow> Rows)> Sections(IReadOnlyList<LauncherRow> rows)
    {
        var result = new List<(string, IReadOnlyList<LauncherRow>)>();
        if (rows == null) return result;
        foreach (var name in rows.Select(r => r.Section).Distinct())
        {
            var inSection = rows.Where(r => r.Section == name).ToList();
            bool IsChild(LauncherRow r) => r.NeedsVar != null && inSection.Any(p => p.Env == r.NeedsVar && p != r);
            var ordered = new List<LauncherRow>();
            void Add(LauncherRow r, int depth)
            {
                ordered.Add(r);
                if (depth > 4) return;
                foreach (var c in inSection.Where(c => c.NeedsVar == r.Env && c != r && !ordered.Contains(c))) Add(c, depth + 1);
            }
            foreach (var r in inSection.Where(r => !IsChild(r) && r.Basic)) Add(r, 0);
            foreach (var r in inSection.Where(r => !IsChild(r) && !r.Basic)) Add(r, 0);
            foreach (var r in inSection.Where(r => !ordered.Contains(r))) ordered.Add(r); // a cycle: still drawn
            result.Add((name, ordered));
        }
        return result;
    }

    private static string FormatRange(double v, double step)
    {
        var decimals = 0;
        for (var s = step; decimals < 6 && Math.Abs(s - Math.Round(s)) > 1e-9; s *= 10) decimals++;
        return Math.Round(v, decimals).ToString("F" + decimals, CultureInfo.InvariantCulture);
    }

    /// A slider position snapped to the row's step and clamped to its range, as the text the file keeps.
    public static string SnapRange(LauncherRow row, double v)
    {
        var snapped = row.Min + Math.Round((v - row.Min) / row.Step) * row.Step;
        snapped = Math.Min(row.Max, Math.Max(row.Min, snapped));
        return FormatRange(snapped, row.Step);
    }

    public static double RangeValue(LauncherRow row, IReadOnlyDictionary<string, string> values)
    {
        var text = ValueOf(row, values);
        return double.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out var d) ? Math.Min(row.Max, Math.Max(row.Min, d)) : row.Min;
    }

    /// "1200×675" for a render scale with a base, in the mode DM_OPMODE says (0 handheld, anything else docked); null without a base.
    public static string RenderSizeText(LauncherRow row, double scale, string opmode)
    {
        if (row.Base == null) return null;
        var handheld = opmode == "0";
        var w = handheld ? row.Base.HandheldW : row.Base.DockedW;
        var h = handheld ? row.Base.HandheldH : row.Base.DockedH;
        return ((int) Math.Round(w * scale)).ToString(CultureInfo.InvariantCulture) + "×" + ((int) Math.Round(h * scale)).ToString(CultureInfo.InvariantCulture);
    }

    /// Whether the value is the row's default (a range compares as numbers).
    public static bool IsDefault(LauncherRow row, string value)
    {
        if (value == row.Default) return true;
        return row.Kind == "range" &&
               double.TryParse(value, NumberStyles.Float, CultureInfo.InvariantCulture, out var a) &&
               double.TryParse(row.Default, NumberStyles.Float, CultureInfo.InvariantCulture, out var b) && Math.Abs(a - b) < 1e-9;
    }

    /// The DM_* values of a settings.tsv, plus the legacy keys translated (mode handheld 0 / docked 1, auto nothing;
    /// fps 30|60 to DM_FPS_TARGET, auto nothing; picture to DM_PICTURE_FIT; show_fps to DM_SHOW_FPS). A DM_* line beats
    /// the legacy line for the same switch. Blank, `#` and malformed lines are ignored; a later line wins.
    public static Dictionary<string, string> ParseSettingValues(string text)
    {
        var legacy = new Dictionary<string, string>();
        var values = new Dictionary<string, string>();
        if (string.IsNullOrEmpty(text)) return values;
        foreach (var raw in text.Split('\n'))
        {
            var line = raw.TrimEnd('\r');
            if (line.Length == 0 || line.TrimStart().StartsWith("#", StringComparison.Ordinal)) continue;
            var tab = line.IndexOf('\t');
            if (tab <= 0) continue;
            var key = line.Substring(0, tab).Trim();
            var value = line.Substring(tab + 1).Trim();
            if (EnvName.IsMatch(key)) { if (value.Length > 0) values[key] = value; continue; }
            var lower = value.ToLowerInvariant();
            switch (key.ToLowerInvariant())
            {
                case "mode": if (lower == "handheld") legacy["DM_OPMODE"] = "0"; else if (lower == "docked") legacy["DM_OPMODE"] = "1"; break;
                case "fps": if (lower == "30" || lower == "60") legacy["DM_FPS_TARGET"] = lower; break;
                case "picture": if (lower == "fit" || lower == "fill") legacy["DM_PICTURE_FIT"] = lower; break;
                case "show_fps": if (lower == "1" || lower == "0") legacy["DM_SHOW_FPS"] = lower; break;
            }
        }
        foreach (var kv in legacy) if (!values.ContainsKey(kv.Key)) values[kv.Key] = kv.Value;
        return values;
    }

    /// The file's text for the values, `KEY<TAB>VALUE\n` per line, and only DM_* keys (the legacy keys are dropped:
    /// one source of truth). With the launcher's rows, a value equal to its row's default is left out (the pipeline and
    /// the engine then use their own default) and the lines follow the rows' order, then any other DM_* key sorted;
    /// without rows every value is written, sorted. A value with a tab or newline in it is not written.
    public static string SerializeSettingValues(IReadOnlyDictionary<string, string> values, IReadOnlyList<LauncherRow> rows = null)
    {
        var sb = new System.Text.StringBuilder();
        if (values == null) return "";
        var done = new HashSet<string>();
        bool Clean(string v) => !string.IsNullOrEmpty(v) && v.IndexOfAny(new[] { '\t', '\n', '\r' }) < 0;
        if (rows != null)
            foreach (var r in rows)
            {
                if (!done.Add(r.Env) || !values.TryGetValue(r.Env, out var v) || !Clean(v) || IsDefault(r, v)) continue;
                sb.Append(r.Env).Append('\t').Append(v).Append('\n');
            }
        foreach (var kv in values.Where(kv => EnvName.IsMatch(kv.Key) && Clean(kv.Value) && !done.Contains(kv.Key)).OrderBy(kv => kv.Key, StringComparer.Ordinal))
            sb.Append(kv.Key).Append('\t').Append(kv.Value).Append('\n');
        return sb.ToString();
    }

    /// `games/<base title id>/launcher.json`, relative to Documents (the engine's settings list for that game, written
    /// by the pipeline after a build); null without a title id.
    public static string LauncherRelativePath(string fileName)
    {
        var id = BaseTitleIdOf(fileName);
        return id == null ? null : "games/" + id + "/launcher.json";
    }

    // ---- progress -------------------------------------------------------------------------------------------

    // Where each of the five stages of gameapp_pipeline.py starts on the bar, from the device timings in
    // docs/STATE-GAMEAPP.md: profile 3.7 s, rewrite ~140 s, shaders none, assemble+sign ~2 s, boot until the
    // guest ends. Equal fifths would park the bar at 20% for two minutes and read as a freeze. The weights only
    // shape the bar: a wrong one makes it less smooth, never wrong. Any other stage count falls back to equal parts.
    // Stage 3 builds the game's shaders on the phone when no library is installed for it (gameapp_pipeline.
    // build_shaders_on_device: ~11 min of translation on a Mac for 22,000 shaders, longer on a phone); with one installed
    // it is instant and the bar simply moves past its share.
    private static readonly double[] StageWeights = { 0.03, 0.45, 0.40, 0.06, 0.06 };

    // The rewrite's modules, in the order gameapp.config lists them, and where each starts inside that stage
    // (main ~125 s of ~140, subsdk0 ~5, sdk ~10).
    private static readonly string[] ModuleOrder = { "main", "subsdk0", "sdk" };
    private static readonly double[] ModuleStart = { 0.00, 0.88, 0.92 };

    private static readonly Regex StageLine = new Regex(@"^stage\s+(\d+)/(\d+):\s*(.*)$", RegexOptions.Singleline);
    private static readonly Regex ModuleCounter = new Regex(@"\((\d+)/(\d+):\s*([A-Za-z0-9_.\-]+)\)");
    private static readonly Regex ShaderCounter = new Regex(@"\((\d+)/(\d+) shaders\)");

    /// Reads one `gameapp.stage` line. Anything that is not `stage k/n: text` is not valid and stands at 0.
    public static StageProgress ParseStage(string line)
    {
        var none = new StageProgress(false, 0, 0, line ?? "", 0.0);
        if (string.IsNullOrWhiteSpace(line)) return none;
        var m = StageLine.Match(line.Trim());
        if (!m.Success) return none;
        if (!int.TryParse(m.Groups[1].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var step) ||
            !int.TryParse(m.Groups[2].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var of) ||
            step < 1 || of < 1 || step > of)
            return none;

        var text = m.Groups[3].Value;
        double start, width;
        if (of == StageWeights.Length)
        {
            start = 0;
            for (int i = 0; i < step - 1; i++) start += StageWeights[i];
            width = StageWeights[step - 1];
        }
        else
        {
            start = (step - 1) / (double) of;
            width = 1.0 / of;
        }
        var fraction = start + width * WithinStage(text);
        return new StageProgress(true, step, of, text, Math.Min(1.0, Math.Max(0.0, fraction)));
    }

    /// How far into its stage a step is, 0..1: only the rewrite says (`(2/3: subsdk0)`), the rest start at 0.
    private static double WithinStage(string text)
    {
        var sc = ShaderCounter.Match(text);
        if (sc.Success &&
            long.TryParse(sc.Groups[1].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var done) &&
            long.TryParse(sc.Groups[2].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var total) && total > 0)
            return Math.Min(1.0, done / (double) total);
        var m = ModuleCounter.Match(text);
        if (!m.Success) return 0.0;
        var index = Array.IndexOf(ModuleOrder, m.Groups[3].Value);
        if (index >= 0) return ModuleStart[index];
        if (int.TryParse(m.Groups[1].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var i) &&
            int.TryParse(m.Groups[2].Value, NumberStyles.None, CultureInfo.InvariantCulture, out var n) &&
            n >= 1 && i >= 1 && i <= n)
            return (i - 1) / (double) n;
        return 0.0;
    }

    /// Whether this `gameapp.stage` line is the boot, the stage where the guest starts drawing: the moment the app puts
    /// the game view on screen. The line is the pipeline's own (`stage 5/5: booting the guest ...`), and only the LAST
    /// stage of five is the boot: a "booting" in another stage's text, or a five-stage line that is not the fifth,
    /// does not bring the game view up over the rewrite.
    public static bool IsBootStage(string stageLine)
    {
        var s = ParseStage(stageLine);
        return s.Valid && s.Of == 5 && s.Step == 5 && s.Text.Contains("booting the guest", StringComparison.Ordinal);
    }

    /// Whether a `gameapp.stage` file was written by the run that is going now. The file still holds the previous
    /// run's last line until the chain's first step overwrites it, and the extraction runs before that step: read
    /// as it stands, the bar would sit at the old run's mark from the first second.
    public static bool IsStageFromThisRun(DateTime fileWrittenAt, DateTime runStartedAt) => fileWrittenAt >= runStartedAt;

    // ---- the previous run, read at launch -------------------------------------------------------------------

    /// How a run that left Documents/gameapp.running behind actually ended.
    /// DESIGNEDEND: its boot ran to the time limit, the shim signalled itself, and the guest's graceful exit finished.
    /// DESIGNEDENDWITHOUTCLEANEXIT: the time limit fired but the guest never logged `graceful exit done` (the exit
    /// hung, or the process was killed during it): not a clean end, and not to be hidden.
    /// FAULTINGUEST: the shim named a fatal signal before re-raising. KILLEDORUNKNOWN: nothing says how it ended.
    /// PLAYEREND: the same clean end, ordered by the player with "Quit game" (the shim's `the player ended the game`
    /// line) instead of by the time limit; the game has no time limit by default now, so this is the usual end.
    public enum PreviousRun { DesignedEnd, PlayerEnd, DesignedEndWithoutCleanExit, FaultInGuest, KilledOrUnknown }

    /// The last line of gameapp.chain.log that belongs to the PREVIOUS run. The shim's launch constructor writes its
    /// census and reservation lines into the same file at the start of THIS process (it does for any launch with a
    /// request or a framework on disk), so at report time they sit at the end of the file, after the line that says
    /// how the previous run ended: read as the last line, they hid the time limit and the fatal signal.
    public static string LastRunLine(IReadOnlyList<string> lines)
    {
        for (int i = (lines?.Count ?? 0) - 1; i >= 0; i--)
        {
            var line = lines[i];
            if (string.IsNullOrWhiteSpace(line)) continue;
            if (line.StartsWith("[gameapp] as-census ", StringComparison.Ordinal) ||
                line.StartsWith("[gameapp] as-reserve:", StringComparison.Ordinal)) continue;
            return line;
        }
        return "(none)";
    }

    /// `stageLine`: gameapp.stage. `chainLast`: LastRunLine of the chain log. `guestLast`: the last line of
    /// gameapp.guest.log (null when there is none). Only a run that reached the boot can have ended at its time limit,
    /// so an older run's time-limit line never excuses a run that died earlier.
    public static PreviousRun ClassifyPreviousRun(string stageLine, string chainLast, string guestLast)
    {
        if (stageLine == null || !stageLine.Contains("booting the guest")) return PreviousRun.KilledOrUnknown;
        bool cleanExit = guestLast != null && guestLast.Contains("graceful exit done");
        // The player's Quit: gameapp_quit_game logs this line before it signals the process (gameapp_shim.c).
        if (chainLast != null && chainLast.Contains("the player ended the game"))
            return cleanExit ? PreviousRun.PlayerEnd : PreviousRun.DesignedEndWithoutCleanExit;
        // "boot time limit N s reached" and "the boot ran to its time limit, the designed end" are the shim's two lines;
        // "boot time limit of N s disarmed" is the OTHER case (the boot returned first, no signal was sent).
        if (chainLast != null && chainLast.Contains("time limit") && !chainLast.Contains("disarmed"))
            return cleanExit ? PreviousRun.DesignedEnd : PreviousRun.DesignedEndWithoutCleanExit;
        if (chainLast != null && chainLast.Contains("received SIG")) return PreviousRun.FaultInGuest;
        return PreviousRun.KilledOrUnknown;
    }

    // ---- the run's status text ------------------------------------------------------------------------------------

    /// `2m 05s`, not `125s`: the chain takes minutes, and a clock that moves in a unit people read is what tells a
    /// slow run from a frozen one.
    public static string FormatElapsed(TimeSpan elapsed)
    {
        long s = (long) Math.Floor(Math.Max(0.0, elapsed.TotalSeconds));
        if (s < 60) return Inv($"{s}s");
        if (s < 3600) return Inv($"{s / 60}m {s % 60:00}s");
        return Inv($"{s / 3600}h {s % 3600 / 60:00}m");
    }

    /// The line under the progress bar while a run is going: the chain's current step and the clock.
    public static string RunningStatus(string lastStage, TimeSpan elapsed) =>
        string.IsNullOrEmpty(lastStage)
            ? Inv($"running: {FormatElapsed(elapsed)}")
            : Inv($"{lastStage}   ({FormatElapsed(elapsed)})");

    /// The line once the run is over.
    public static string DoneStatus(TimeSpan elapsed) => Inv($"done in {elapsed.TotalSeconds:F1}s");

    // ---- the build screen: one row per step, done / now / next, with its time -------------------------------------

    public enum StepState { Done, Now, Next }

    public sealed record BuildStep(string Name, StepState State, string Time, string Detail, double Fraction);

    /// The six steps a build shows, in order: the extraction (the app's, before the chain writes any stage line), then
    /// the chain's five stages (gameapp.stage: `stage k/5: ...`).
    public static readonly string[] StepNames = { "Read the game", "Prepare the profile", "Rewrite the code for iPhone", "Translate the shaders", "Sign the game", "Launch" };

    /// The rows, from the current stage line (null while the extraction runs) and when each step started (index 0..5,
    /// filled by the screen as it sees each step begin; missing = not seen). `now` is the clock.
    public static List<BuildStep> BuildSteps(string stageLine, IReadOnlyDictionary<int, DateTime> startedAt, DateTime now, bool finished = false)
    {
        var s = ParseStage(stageLine);
        int current = s.Valid && s.Of == 5 ? s.Step : 0;   // 0: still extracting
        if (finished) current = 6;
        var rows = new List<BuildStep>();
        for (int i = 0; i < StepNames.Length; i++)
        {
            var state = i < current ? StepState.Done : i == current ? StepState.Now : StepState.Next;
            string time = "";
            if (startedAt != null && startedAt.TryGetValue(i, out var t0))
            {
                DateTime end = now;
                if (state == StepState.Done)
                    end = startedAt.TryGetValue(i + 1, out var t1) ? t1 : now;
                if (state != StepState.Next) time = Clock(end - t0);
            }
            string detail = "";
            double fraction = 0;
            if (state == StepState.Now && i == 3)
            {
                var sc = ShaderCounter.Match(s.Text);
                if (sc.Success) { detail = Inv($"{long.Parse(sc.Groups[1].Value):N0} / {long.Parse(sc.Groups[2].Value):N0} shaders"); fraction = WithinStage(s.Text); }
                else if (s.Text.Contains("already here")) detail = "already translated for this game";
                else detail = "reading the game's shaders";
            }
            else if (state == StepState.Now && i == 2)
            {
                var m = ModuleCounter.Match(s.Text);
                if (m.Success) { detail = Inv($"module {m.Groups[1].Value} of {m.Groups[2].Value}: {m.Groups[3].Value}"); fraction = WithinStage(s.Text); }
            }
            else if (state == StepState.Now && i == 4)
                detail = s.Text.Contains("signing") ? "signing" : "assembling";
            rows.Add(new BuildStep(StepNames[i], state, time, detail, fraction));
        }
        return rows;
    }

    /// The time a build has left, or null when it cannot be told yet: while the shaders are translated, from their own
    /// pace since that step began (it is most of a build); else from the whole bar's pace once it is past 5 %.
    public static TimeSpan? TimeLeft(string stageLine, IReadOnlyDictionary<int, DateTime> startedAt, DateTime now, TimeSpan elapsed)
    {
        var s = ParseStage(stageLine);
        if (s.Valid && s.Of == 5 && s.Step == 3 && startedAt != null && startedAt.TryGetValue(3, out var t3))
        {
            var sc = ShaderCounter.Match(s.Text);
            if (sc.Success && long.TryParse(sc.Groups[1].Value, out var done) && long.TryParse(sc.Groups[2].Value, out var total) && done >= 20 && total > done)
            {
                var spent = (now - t3).TotalSeconds;
                if (spent > 5) return TimeSpan.FromSeconds(spent / done * (total - done));
            }
        }
        if (s.Valid && s.Fraction >= 0.05 && s.Fraction < 1 && elapsed.TotalSeconds > 10)
            return TimeSpan.FromSeconds(elapsed.TotalSeconds * (1 - s.Fraction) / s.Fraction);
        return null;
    }

    /// "~3 min", "~1 h 05", "under a minute".
    public static string LeftText(TimeSpan? left)
    {
        if (left == null) return "—";
        var t = left.Value;
        if (t.TotalSeconds < 60) return "under a minute";
        if (t.TotalMinutes < 60) return Inv($"~{Math.Ceiling(t.TotalMinutes):0} min");
        return Inv($"~{(int) t.TotalHours} h {t.Minutes:00}");
    }

    /// `4:12`, `0:02`, `1:03:20`: a step's time, as the build screen shows it.
    public static string Clock(TimeSpan t)
    {
        long s = (long) Math.Max(0, Math.Floor(t.TotalSeconds));
        return s >= 3600 ? Inv($"{s / 3600}:{s % 3600 / 60:00}:{s % 60:00}") : Inv($"{s / 60}:{s % 60:00}");
    }

    // ---- the log: each line's kind, for the timeline and its errors-only switch ------------------------------------

    public enum LogKind { Info, Stage, Warning, Error }

    /// A line's kind. Errors: the chain's [RED] lines, a FAILED, an exception; warnings: a warning, a refused or
    /// skipped thing, an [ORANGE] line; stages: the chain's `stage k/n:` lines and the app's run banners.
    public static LogKind KindOfLine(string line)
    {
        var l = (line ?? "").Trim();
        if (l.Length == 0) return LogKind.Info;
        if (StageLine.IsMatch(l) || l.StartsWith("=== ", StringComparison.Ordinal)) return LogKind.Stage;
        if (l.Contains("[RED]") || l.Contains("FAILED") || l.Contains("Traceback") || l.Contains("Exception:") || l.StartsWith("error", StringComparison.OrdinalIgnoreCase)
            || l.Contains(" error:") || l.Contains("could not be translated"))
            return LogKind.Error;
        if (l.Contains("[ORANGE]") || l.Contains("warning", StringComparison.OrdinalIgnoreCase) || l.Contains("[WARN]") || l.Contains("refused") || l.Contains("MISSING"))
            return LogKind.Warning;
        return LogKind.Info;
    }

    /// One row of the log's timeline: its time, the step it opens (a header shown above it, or null), its text, its kind.
    public sealed record TimelineRow(string Time, string Header, string Text, LogKind Kind);

    /// The log as the timeline shows it (design C): a header each time the build moves to another of its six steps (the
    /// step's own stage lines -- one per shader count -- are not rows: the build screen shows them), a "New run" header
    /// at a run's banner, "App" before anything; with `onlyProblems`, only errors and warnings (a header still opens
    /// the first one shown of its step).
    public static List<TimelineRow> Timeline(IEnumerable<(string Time, string Text, LogKind Kind)> lines, bool onlyProblems)
    {
        var rows = new List<TimelineRow>();
        string pending = "App";
        int step = -1;
        foreach (var (time, text, kind) in lines ?? Enumerable.Empty<(string, string, LogKind)>())
        {
            var t = (text ?? "").Trim();
            if (kind == LogKind.Stage)
            {
                var s = ParseStage(t);
                if (s.Valid && s.Of == 5)
                {
                    if (s.Step != step) { pending = StepNames[Math.Min(s.Step, StepNames.Length - 1)]; step = s.Step; }
                    continue;
                }
                if (t.StartsWith("=== ", StringComparison.Ordinal))
                {
                    pending = "New run";
                    step = 0;
                    if (onlyProblems) continue;
                    rows.Add(new TimelineRow(time, pending, t.Substring(4), LogKind.Stage));
                    pending = null;
                    continue;
                }
            }
            if (onlyProblems && kind != LogKind.Error && kind != LogKind.Warning) continue;
            t = Tidy(t);
            // A counter ("compiling: 758/21990 shaders", "NAS: 40 % of x.ipa") is one row that moves: a line with the
            // same label as the row just before replaces it (time and count), instead of one row a second.
            var label = ProgressLabel(t);
            if (label != null && rows.Count > 0 && pending == null && ProgressLabel(rows[^1].Text) == label && rows[^1].Kind == kind)
            {
                rows[^1] = rows[^1] with { Time = time, Text = t };
                continue;
            }
            rows.Add(new TimelineRow(time, pending, t, kind));
            pending = null;
        }
        return rows;
    }

    private static readonly Regex Counter = new Regex(@"^(?<label>.{0,80}?)\s*\(?\d[\d,]*\s*/\s*\d[\d,]*\)?\s*(?<unit>[A-Za-z]*)\b|^(?<label>.{1,80}?)\s+\d{1,3}\s*%\s+of\b", RegexOptions.Compiled);

    /// The part of a counter line that stays the same from one count to the next ("compiling: |shaders"), or null when
    /// the line counts nothing.
    public static string ProgressLabel(string line)
    {
        var m = Counter.Match(line ?? "");
        return m.Success ? m.Groups["label"].Value.Trim() + "|" + m.Groups["unit"].Value : null;
    }

    /// A line as the timeline shows it: the chain's colour tags go (the row's colour says it).
    public static string Tidy(string line)
    {
        var t = (line ?? "").Replace("[RED] ", "").Replace("[RED]", "").Replace("[ORANGE] ", "").Replace("[ORANGE]", "").Trim();
        // The app's own folders, as the screen says them: ".../Containers/Data/Application/<uuid>/Documents/x" is
        // "Documents/x", the bundle's ".../Bundle/Application/<uuid>/GameApp.app/x" is "app/x" (the shared logs keep them whole).
        t = DataFolder.Replace(t, "");
        t = BundleFolder.Replace(t, "app/");
        return t.Length > 500 ? t.Substring(0, 500) + " …" : t;
    }

    private static readonly Regex DataFolder = new Regex(@"[^\s'""(]*/Containers/(?:Shared/AppGroup|Data/Application)/[0-9A-Fa-f-]{36}/", RegexOptions.Compiled);
    private static readonly Regex BundleFolder = new Regex(@"[^\s'""(]*/Containers/Bundle/Application/[0-9A-Fa-f-]{36}/[^/\s'""]+\.app/", RegexOptions.Compiled);

    /// Errors and warnings in a block of log text.
    public static (int Errors, int Warnings) CountProblems(IEnumerable<string> lines)
    {
        int e = 0, w = 0;
        foreach (var line in lines ?? Enumerable.Empty<string>())
        {
            var k = KindOfLine(line);
            if (k == LogKind.Error) e++;
            else if (k == LogKind.Warning) w++;
        }
        return (e, w);
    }

    // ---- the storage bar: games, builds, shaders, free -------------------------------------------------------

    public sealed record StorageSegment(string Label, long Bytes);

    /// The bar's parts, in order, and the free space last (each at least shown when it holds anything).
    public static List<StorageSegment> StorageSegments(long games, long builds, long shaders, long exports, long free)
    {
        var list = new List<StorageSegment> { new("Games", games), new("Builds", builds), new("Shaders", shaders) };
        if (exports > 0) list.Add(new("Apps", exports));
        list.Add(new("Free", Math.Max(0, free)));
        return list;
    }

    private static string Inv(FormattableString text) => text.ToString(CultureInfo.InvariantCulture);
}
