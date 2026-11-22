// gameapp/GameAppResume.cs -- a build iOS ended, and a build the app ends on purpose (docs/DECISIONS.md traps 74 and 75).
//
//   * A build that never finished (Documents/gameapp.running is still there at launch) and was not in the boot: the app
//     writes Documents/gameapp.killed (the game, the last stage, the last memory line), shows "iOS ended the last build
//     (memory). Resume" on the library, and Resume starts the same game's chain again on the files already extracted.
//     Stage 3 resumes from what it finished (gameapp_pipeline.ShaderResume); the pipeline puts the note's last stage and last
//     memory line into the new run's log and takes the note away.
//   * Between the rewrite and the shaders the pipeline may end the chain on purpose (result 75) after writing
//     Documents/gameapp.continue: the free-threaded runtime keeps the memory the rewrite's threads freed, and only a new
//     process has it back. iOS cannot start an app by itself, so the app posts "Tap to continue the build", closes, and the
//     next launch starts that game's chain again on its own (the rewrite is in the cache, so it costs seconds).
using System;
using System.IO;
using System.Linq;
using System.Text.RegularExpressions;
using System.Threading.Tasks;

using Foundation;
using UIKit;

public sealed partial class GameAppViewController
{
    /// The pipeline's result when it ends the chain on purpose to be restarted (gameapp_pipeline.RESTART_RC, EX_TEMPFAIL).
    private const int ChainRestartResult = 75;

    private static string KilledPath => Path.Combine(GameAppMain.Documents, "gameapp.killed");
    private static string ContinuePath => Path.Combine(GameAppMain.Documents, "gameapp.continue");
    private static string ContinueRunPath => ContinuePath + ".run";

    private static string KeyOf(string text, string key)
    {
        foreach (var line in (text ?? "").Split('\n'))
        {
            var at = line.IndexOf('=');
            if (at > 0 && line.Substring(0, at).Trim() == key) return line.Substring(at + 1).Trim();
        }
        return "";
    }

    /// The stage number of a pipeline stage line ("stage 3/5: ..."), 0 when there is none.
    internal static int StageNumber(string stageLine)
    {
        var m = Regex.Match(stageLine ?? "", @"^stage (\d)/\d");
        return m.Success ? int.Parse(m.Groups[1].Value) : 0;
    }

    /// An interrupted run that was a BUILD (not a boot): remembered for the library's Resume row and for the next run's log.
    /// `markerText` is Documents/gameapp.running (the start time; "game=<file>" and "bootonly=1" from BeginRun), `stageLine`
    /// the last line of gameapp.stage. A stage file older than the run's start is the previous run's: the build was killed
    /// before the chain wrote its first stage line, in the extraction (stage 0).
    private void RecordKilledBuild(string markerText, string stageLine, string started)
    {
        try
        {
            var game = KeyOf(markerText, "game");
            if (KeyOf(markerText, "bootonly") == "1" || game.Length == 0 || !File.Exists(Path.Combine(GameAppMain.Documents, game)))
            {
                if (File.Exists(KilledPath)) File.Delete(KilledPath);
                return;
            }
            int stage = StageNumber(stageLine);
            var stagePath = Path.Combine(GameAppMain.Documents, "gameapp.stage");
            bool fresh = DateTime.TryParse(started.Split(' ')[0], null, System.Globalization.DateTimeStyles.RoundtripKind, out var at)
                         && File.Exists(stagePath) && File.GetLastWriteTime(stagePath) >= at;
            if (!fresh) { stage = 0; stageLine = "(the extraction: the chain had not written a stage yet)"; }
            if (stage >= 5) { if (File.Exists(KilledPath)) File.Delete(KilledPath); return; }   // the boot: a game's end, not a build's
            File.WriteAllText(KilledPath,
                $"game={game}\nstagen={stage}\nstage={stageLine}\nmem={LastMatching("pipeline.stages.log", "[mem:").Trim()}\nstarted={started}\n");
            Write($"  iBuilder offers to resume the build of {game} (Documents/gameapp.killed)");
        }
        catch (Exception error) { Write($"could not record the killed build: {error.GetType().Name}: {error.Message}"); }
    }

    /// The library's row for a build iOS ended: tap, then Resume / Dismiss. Called by RefreshGamesPage before it sizes the banner.
    private void AddKilledBanner()
    {
        if (running || !File.Exists(KilledPath)) return;
        string text;
        try { text = File.ReadAllText(KilledPath); } catch (Exception) { return; }
        var game = KeyOf(text, "game");
        if (game.Length == 0 || !File.Exists(Path.Combine(GameAppMain.Documents, game))) { try { File.Delete(KilledPath); } catch (Exception) { } return; }
        var font = UIFont.GetPreferredFontForTextStyle(UIFontTextStyle.Footnote);
        var bold = UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(UIFontDescriptorSymbolicTraits.Bold), font.PointSize);
        var label = new UILabel { Lines = 0, AdjustsFontForContentSizeCategory = true };
        var line = new NSMutableAttributedString();
        line.Append(new NSAttributedString("! ", new UIStringAttributes { Font = bold, ForegroundColor = UIColor.SystemOrange }));
        line.Append(new NSAttributedString("iOS ended the last build (memory).  ", new UIStringAttributes { Font = bold, ForegroundColor = UIColor.Label }));
        line.Append(new NSAttributedString("Resume", new UIStringAttributes { Font = bold, ForegroundColor = UIColor.SystemBlue }));
        label.AttributedText = line;
        label.AccessibilityLabel = "iOS ended the last build (memory). Resume";
        var view = new BannerView(label, UIColor.SystemOrange) { AccessibilityLabel = "iOS ended the last build (memory). Resume", AccessibilityIdentifier = "banner-resume" };
        view.TouchUpInside += (_, __) => AskToResume(game, text);
        banner.AddArrangedSubview(view);
    }

    private void AskToResume(string game, string note)
    {
        var stage = KeyOf(note, "stage");
        var alert = UIAlertController.Create($"Resume {GameAppUi.DisplayName(game)}?",
            $"iOS ended the build for memory at: {(stage.Length > 0 ? stage : "an unknown step")}. Resume continues from what was finished: the rewrite and the shaders already translated are not done again.",
            UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("Resume", UIAlertActionStyle.Default, _ => ResumeKilledBuild(game, int.TryParse(KeyOf(note, "stagen"), out var n) ? n : 0)));
        alert.AddAction(UIAlertAction.Create("Dismiss", UIAlertActionStyle.Destructive, _ =>
        {
            try { File.Delete(KilledPath); } catch (Exception) { }
            RefreshScreen();
        }));
        alert.AddAction(UIAlertAction.Create("Not now", UIAlertActionStyle.Cancel, null));
        PresentViewController(alert, true, null);
    }

    /// Resume: the same game's chain again. An NSP/XCI game was extracted before its chain wrote a stage, so the extraction is not
    /// repeated; a .nup is read by the chain itself, which skips that when the killed run had got past it (stage 2 and later).
    private void ResumeKilledBuild(string game, int stage)
    {
        if (running) return;
        if (!GameAppUi.IsNup(game) && stage < 1) { SetUpAndPlay(game); return; }   // killed in the extraction: all of it again
        if (GameAppUi.IsNup(game) && stage < 2) { SetUpAndPlay(game); return; }
        StartContinuedBuild(game, stage, "killed");
    }

    private void StartContinuedBuild(string game, int stage, string why)
    {
        if (running || pythonRanThisProcess) return;
        var path = Path.Combine(GameAppMain.Documents, game);
        if (!File.Exists(path)) { Write($"{game} is no longer in Documents: nothing to continue"); try { File.Delete(ContinuePath); File.Delete(KilledPath); } catch (Exception) { } return; }
        try
        {
            // the chain reads this (take_continue) and the request is used once; a pending boot request would make the chain only boot
            File.WriteAllText(ContinueRunPath, $"game={game}\nstage={stage}\nwhy={why}\n");
            if (File.Exists(ContinuePath)) File.Delete(ContinuePath);
            if (File.Exists(BootOnlyPath)) File.Delete(BootOnlyPath);
        }
        catch (Exception error) { Write($"could not record the continuation: {error.GetType().Name}: {error.Message}"); return; }
        var updateName = GameAppUi.UpdateFor(game, Dumps().Select(Path.GetFileName));
        var arguments = new[]
        {
            Path.Combine(GameAppMain.Documents, "prod.keys"), path, updateName != null ? Path.Combine(GameAppMain.Documents, updateName) : "-",
            Path.Combine(GameAppMain.Documents, "extract"), "--romfs-image",
        };
        Write($"continuing the build of {game} ({why}): the chain runs again on the files already extracted");
        BeginRun(game, arguments, skipExtract: true);
    }

    /// At launch: a build the last process ended on purpose before stage 3 (Documents/gameapp.continue) starts again by itself.
    /// Returns true when it started one.
    private bool ContinueBuildAtLaunch()
    {
        if (!File.Exists(ContinuePath)) return false;
        string text = "";
        try { text = File.ReadAllText(ContinuePath); } catch (Exception) { }
        var game = KeyOf(text, "game");
        if (game.Length == 0) { try { File.Delete(ContinuePath); } catch (Exception) { } return false; }
        Write($"gameapp.continue present — the last process ended on purpose to give the rewrite's memory back; this launch continues the build of {game} from stage 3");
        StartContinuedBuild(game, int.TryParse(KeyOf(text, "stage"), out var s) ? s : 3, "memory between the rewrite and the shaders");
        return running;
    }

    /// The chain ended with its restart result: tell the user ("Tap to continue the build") and close, so the next launch has the
    /// memory the rewrite's threads freed. Returns true when it took over the end of the run (the caller skips Finish).
    private bool RestartForStage3(int rc, string chainError)
    {
        if (rc != ChainRestartResult || chainError != null || !File.Exists(ContinuePath)) return false;
        var game = KeyOf(File.ReadAllText(ContinuePath), "game");
        TailLogs();
        running = false;
        try { File.Delete(Path.Combine(GameAppMain.Documents, "gameapp.running")); } catch (Exception) { }   // an end by design, not a kill
        StopTicker();
        BackgroundEnd(true);   // GameAppBackground.cs: the process ends next, and the task with it
        Write("the chain ended on purpose before stage 3: iBuilder closes so the shaders have the memory the rewrite held; open it again and the build continues by itself");
        var name = game.Length > 0 ? GameAppUi.DisplayName(game) : "iBuilder";
        UserNotifications.UNUserNotificationCenter.Current.RequestAuthorization(
            UserNotifications.UNAuthorizationOptions.Alert | UserNotifications.UNAuthorizationOptions.Sound, (granted, _) =>
                BeginInvokeOnMainThread(() =>
                {
                    if (!granted)
                    {
                        var alert = UIAlertController.Create($"{name}: the build continues in a moment",
                            "iBuilder closes to give the shaders the memory the rewrite held. Open it again and the build continues by itself.",
                            UIAlertControllerStyle.Alert);
                        alert.AddAction(UIAlertAction.Create("Close", UIAlertActionStyle.Default, __ => EndProcess()));
                        PresentWhenFree(alert);
                        return;
                    }
                    var content = new UserNotifications.UNMutableNotificationContent
                    {
                        Title = name,
                        Body = "iBuilder closed to free memory. Tap to continue the build.",
                        Sound = UserNotifications.UNNotificationSound.Default,
                    };
                    var trigger = UserNotifications.UNTimeIntervalNotificationTrigger.CreateTrigger(1, false);
                    var request = UserNotifications.UNNotificationRequest.FromIdentifier("build-continue", content, trigger);
                    UserNotifications.UNUserNotificationCenter.Current.AddNotificationRequest(request, __ => Task.Delay(400).ContinueWith(___ => EndProcess()));
                }));
        return true;
    }

    /// SIGTERM: the app's own clean end, as Quit game and AskToRestartForGame use.
    private static void EndProcess() => kill(getpid(), 15);
}
