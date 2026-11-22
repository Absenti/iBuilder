// The build queue, the heat check and the thermal file: the screens and the steps around GameAppQueue.cs's logic.
//
//   Queue    Library ··· › "Add to the build queue"; Settings › Build queue lists, removes and starts. Documents/build_queue.tsv
//            keeps it across launches. iOS runs a build's Python once per process, so after each build the app asks to
//            close and the NEXT game builds at the next launch (a notification reminds); each build goes on in the background (GameAppBackground.cs)
//            but the next game needs the app opened, and the screen stays awake while it is open.
//   Thermal  while a build runs, Documents/gameapp.thermal says "<state> <lowperf>" every 3 s for the pipeline.
//   Heat     before a build or Play on a hot phone: "wait to cool down?", and the build screen shows the heat.

using System;
using System.IO;
using System.Linq;
using Foundation;
using UIKit;

public sealed partial class GameAppViewController
{
    // ---- the queue ------------------------------------------------------------------------------------------------

    private BuildQueueState buildQueue = new BuildQueueState();
    private bool queueWanted;      // "Start the queue" was tapped (or the last launch handed the turn over)
    private string queueGame;      // the game of the build that is running for the queue
    private readonly UIStackView queueList = new UIStackView();
    private UILabel queueNote, queueDetail, queueStatus;
    private UIView queueCard;
    private UIButton queueStart;

    private static string QueuePath => Path.Combine(GameAppMain.Documents, "build_queue.tsv");

    private void LoadBuildQueue()
    {
        try { buildQueue = BuildQueue.Parse(File.Exists(QueuePath) ? File.ReadAllText(QueuePath) : ""); }
        catch (Exception) { buildQueue = new BuildQueueState(); }
        // A queued build the last process ended on purpose before stage 3 continues at this launch (ContinueBuildAtLaunch):
        // it is still the queue's, so its end moves the queue on.
        string continuing = null;
        try { if (File.Exists(ContinuePath)) continuing = KeyOf(File.ReadAllText(ContinuePath), "game"); } catch (Exception) { }
        if (continuing != null && buildQueue.Items.Any(i => i.State == BuildQueue.Running && i.Game == continuing))
        {
            queueGame = continuing;
            queueWanted = true;
        }
        if (BuildQueue.ResolveInterrupted(buildQueue, queueGame))
        {
            SaveBuildQueue();
            Write("build queue: a game was in its build when the app closed: it is marked, and nothing starts by itself");
        }
    }

    private void SaveBuildQueue()
    {
        try
        {
            if (buildQueue.Items.Count == 0 && !buildQueue.AutoStart) { if (File.Exists(QueuePath)) File.Delete(QueuePath); }
            else File.WriteAllText(QueuePath, BuildQueue.Format(buildQueue));
        }
        catch (Exception error) { Write($"build queue: could not be saved: {error.Message}"); }
        RefreshQueuePage();
    }

    private void AddToBuildQueue(string game)
    {
        if (!BuildQueue.Add(buildQueue, game)) { Write($"build queue: {GameAppUi.DisplayName(game)} is already in it"); return; }
        SaveBuildQueue();
        Write($"build queue: {GameAppUi.DisplayName(game)} added ({buildQueue.Queued} waiting). Settings › Build queue starts it.");
        var a = UIAlertController.Create("Added to the build queue",
            $"{GameAppUi.DisplayName(game)} waits in Settings › Build queue. The queue builds one game after another; each build keeps running if you leave iBuilder, but the next game starts when you open it again.",
            UIAlertControllerStyle.Alert);
        a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
        a.AddAction(UIAlertAction.Create("Open the queue", UIAlertActionStyle.Default, _ => ShowTab(6)));
        PresentWhenFree(a);
    }

    private void BuildQueuePage()
    {
        queueList.Axis = UILayoutConstraintAxis.Vertical;
        queueList.Spacing = 2;
        var header = SubHeader("Build queue");
        queueNote = Theme.Label("The games build one after another. A build keeps running when you leave iBuilder (its progress shows on the lock screen); the screen stays awake while iBuilder is open. "
                                + "iOS starts a build once per launch, so after each game iBuilder closes and the next one builds when you open it again (a notification reminds you).",
                                Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0);
        queueCard = Theme.Wrap(queueList, 22, 10);
        queueStatus = Theme.Label("", Theme.Font(13, UIFontWeight.Medium), Theme.Muted, 0);
        queueStart = Theme.Pill("Start the queue", "play.fill", true, 56);
        queueStart.AccessibilityIdentifier = "queue-start";
        queueStart.TouchUpInside += (_, __) => ToggleQueue();
        var column = Theme.Column(14, header, queueNote, queueCard, queueStatus, queueStart);
        var scroll = new UIScrollView { AlwaysBounceVertical = true };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        Theme.PinColumn(scroll, column, 4, 20);
        Fill(pages[6], scroll, 18);
        RefreshQueuePage();
    }

    private void RefreshQueuePage()
    {
        if (queueDetail != null)
            queueDetail.Text = buildQueue.Items.Count == 0 ? "Empty" : buildQueue.Queued > 0 ? $"{buildQueue.Queued} waiting" : "Stopped";
        if (queueCard == null) return;
        foreach (var v in queueList.ArrangedSubviews.ToArray()) { queueList.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        var facts = buildQueue.Items.Count > 0 ? GatherFacts() : null;
        bool first = true;
        foreach (var item in buildQueue.Items.ToList())
        {
            var game = item.Game;
            string sub = item.State == BuildQueue.Running ? "Building now"
                       : item.State == BuildQueue.Failed ? "Stopped: " + (string.IsNullOrEmpty(item.Note) ? "see the log" : item.Note)
                       : first ? "Next" : "Waiting";
            if (item.State == BuildQueue.Queued) first = false;
            var row = new GameRowView(CoverOf(game, facts), GameAppUi.DisplayName(game), sub, false, false, null,
                                      item.State == BuildQueue.Running ? null : "xmark", false, null);
            row.Trailing.AccessibilityLabel = "Remove " + GameAppUi.DisplayName(game);
            row.Trailing.TouchUpInside += (_, __) =>
            {
                BuildQueue.Remove(buildQueue, game);
                SaveBuildQueue();
            };
            row.AccessibilityIdentifier = "queue-row-" + queueList.ArrangedSubviews.Length;
            queueList.AddArrangedSubview(row);
        }
        queueCard.Hidden = buildQueue.Items.Count == 0;
        var c = queueStart.Configuration;
        var text = queueWanted ? "Stop the queue" : "Start the queue";
        c.AttributedTitle = new Foundation.NSAttributedString(text, new UIStringAttributes { Font = Theme.Font(16, UIFontWeight.Semibold) });
        c.Image = UIImage.GetSystemImage(queueWanted ? "stop.fill" : "play.fill");
        queueStart.Configuration = c;
        queueStart.Enabled = queueWanted || BuildQueue.Next(buildQueue) != null;
        queueStatus.Text = buildQueue.Items.Count == 0 ? "Nothing queued: a game's ··· menu in the library has Add to the build queue."
                         : coolWaiter != null ? $"Waiting for the {GameAppUi.DeviceName} to cool down: the next build starts by itself."
                         : queueWanted && running ? "Building. The next game starts after this one."
                         : "";
    }

    private void ToggleQueue()
    {
        if (queueWanted)
        {
            queueWanted = false;
            buildQueue.AutoStart = false;
            UIApplication.SharedApplication.IdleTimerDisabled = false;
            SaveBuildQueue();
            Write("build queue: stopped (a build that is running finishes)");
            return;
        }
        if (BuildQueue.Next(buildQueue) == null) return;
        queueWanted = true;
        UIApplication.SharedApplication.IdleTimerDisabled = true;   // the queue runs only while the app is open
        UserNotifications.UNUserNotificationCenter.Current.RequestAuthorization(
            UserNotifications.UNAuthorizationOptions.Alert | UserNotifications.UNAuthorizationOptions.Sound, (granted, error) => { });
        Write("build queue: started");
        RefreshQueuePage();
        if (!running) QueueStartNext();
    }

    /// The first launch frame is up: when the last build handed the turn over, the next game builds now.
    private void QueueResumeIfOwed()
    {
        if (!buildQueue.AutoStart || running || BuildQueue.Next(buildQueue) == null) return;
        buildQueue.AutoStart = false;
        queueWanted = true;
        UIApplication.SharedApplication.IdleTimerDisabled = true;
        SaveBuildQueue();
        Write("build queue: the next game builds now (the previous one ended and the app was reopened)");
        QueueStartNext();
    }

    private void QueueStartNext()
    {
        var item = BuildQueue.Next(buildQueue);
        if (item == null) { QueueFinished(); return; }
        if (running) return;
        if (pythonRanThisProcess) { QueueAskRestart(item); return; }
        if (BuildHeat.ShouldAsk(ThermalState()))
        {
            Write($"build queue: the iPhone is hot: {GameAppUi.DisplayName(item.Game)} starts when it has cooled down");
            WhenCool(QueueStartNext);
            return;
        }
        var path = Path.Combine(GameAppMain.Documents, item.Game);
        if (!File.Exists(path))
        {
            BuildQueue.Ended(buildQueue, item.Game, false, "the game file is no longer in iBuilder");
            SaveBuildQueue();
            QueueStartNext();
            return;
        }
        BuildQueue.MarkRunning(buildQueue, item.Game);
        queueGame = item.Game;
        SaveBuildQueue();
        selected = item.Game;
        Write($"build queue: building {GameAppUi.DisplayName(item.Game)} ({buildQueue.Queued} more waiting)");
        heatBypass = true;
        try { SetUpAndPlay(item.Game); } finally { heatBypass = false; }
        if (!running)
        {
            // Refused before it began (no keys, a withdrawn file): the reason is in the log; the queue goes on.
            queueGame = null;
            BuildQueue.Ended(buildQueue, item.Game, false, "it did not start: see the log");
            SaveBuildQueue();
            if (BuildQueue.Next(buildQueue) != null) QueueStartNext(); else QueueFinished();
        }
    }

    /// Called from Finish(): a queued build ended. A good one leaves the queue, a failed one stays marked, then the
    /// turn goes to the next game (a launch of its own) or, with none left, the queue is done.
    private void QueueBuildEnded()
    {
        if (queueGame != null)
        {
            BuildQueue.Ended(buildQueue, queueGame, !failedRun, failedRun ? "the build stopped: see the log" : "");
            queueGame = null;
            SaveBuildQueue();
        }
        else if (!queueWanted) return;
        if (queueWanted && BuildQueue.Next(buildQueue) != null) QueueAskRestart(BuildQueue.Next(buildQueue));
        else if (queueWanted) QueueFinished();
    }

    private void QueueAskRestart(QueueItem next)
    {
        buildQueue.AutoStart = true;
        SaveBuildQueue();
        var name = GameAppUi.DisplayName(next.Game);
        var content = new UserNotifications.UNMutableNotificationContent
        {
            Title = "Build queue",
            Body = $"Open iBuilder to build {name} ({buildQueue.Queued} waiting).",
            Sound = UserNotifications.UNNotificationSound.Default,
        };
        UserNotifications.UNUserNotificationCenter.Current.AddNotificationRequest(
            UserNotifications.UNNotificationRequest.FromIdentifier("build-queue-next", content, null), null);
        var a = UIAlertController.Create("Next in the build queue",
            $"{name} is next. iOS builds once per launch: iBuilder closes now, and the build starts when you open it again (a notification reminds you).",
            UIAlertControllerStyle.Alert);
        a.AddAction(UIAlertAction.Create("Close and continue", UIAlertActionStyle.Default, _ =>
        {
            Write("build queue: the app closes; the next game builds at the next launch");
            kill(getpid(), 15);
        }));
        a.AddAction(UIAlertAction.Create("Stop the queue", UIAlertActionStyle.Cancel, _ =>
        {
            queueWanted = false;
            buildQueue.AutoStart = false;
            UIApplication.SharedApplication.IdleTimerDisabled = false;
            SaveBuildQueue();
        }));
        PresentWhenFree(a);
    }

    private void QueueFinished()
    {
        bool failed = buildQueue.Items.Any(i => i.State == BuildQueue.Failed);
        queueWanted = false;
        buildQueue.AutoStart = false;
        UIApplication.SharedApplication.IdleTimerDisabled = false;
        SaveBuildQueue();
        Write("build queue: done" + (failed ? " (some games stopped: Settings › Build queue)" : ""));
        var content = new UserNotifications.UNMutableNotificationContent
        {
            Title = "Build queue done",
            Body = failed ? "The queue is finished; some games stopped: open iBuilder to see why." : "Every game in the queue is built.",
            Sound = UserNotifications.UNNotificationSound.Default,
        };
        UserNotifications.UNUserNotificationCenter.Current.AddNotificationRequest(
            UserNotifications.UNNotificationRequest.FromIdentifier("build-queue-done", content, null), null);
    }

    // ---- heat, and Low performance ------------------------------------------------------------------------------------

    private bool heatBypass;
    private Action coolWaiter;
    private NSObject thermalObserver;
    private NSTimer thermalTimer;

    private static int ThermalState() => (int) NSProcessInfo.ProcessInfo.ThermalState;

    private static string ThermalPath => Path.Combine(GameAppMain.Documents, "gameapp.thermal");

    /// The build screen's quiet heat note: nothing while nominal.
    private static string HeatSuffix()
    {
        var chip = BuildHeat.Chip(ThermalState());
        return chip == null ? "" : " · " + GameAppUi.DeviceName + " " + chip;
    }

    /// Before a build or Play: when the phone is already hot, asks whether to wait. True when it asked (the caller stops;
    /// `again` runs on either answer, at once or when the phone is back to fair).
    private bool AskIfHot(Action again)
    {
        if (heatBypass || !BuildHeat.ShouldAsk(ThermalState())) return false;
        var a = UIAlertController.Create("Heat check", $"Your {GameAppUi.DeviceName} is hot: wait to cool down? It will run slower until then.", UIAlertControllerStyle.Alert);
        a.AddAction(UIAlertAction.Create("Start anyway", UIAlertActionStyle.Destructive, _ =>
        {
            heatBypass = true;
            try { again(); } finally { heatBypass = false; }
        }));
        a.AddAction(UIAlertAction.Create("Wait", UIAlertActionStyle.Default, _ => WhenCool(again)));
        PresentWhenFree(a);
        return true;
    }

    /// Runs `start` when the thermal state is back to fair or nominal (now, if it already is).
    private void WhenCool(Action start)
    {
        if (BuildHeat.CoolEnough(ThermalState())) { start(); return; }
        coolWaiter = start;
        Write("heat: waiting for the iPhone to cool down; it starts by itself at fair or nominal");
        RefreshQueuePage();
        thermalObserver ??= NSNotificationCenter.DefaultCenter.AddObserver(NSProcessInfo.ThermalStateDidChangeNotification, _ =>
        {
            if (coolWaiter == null || !BuildHeat.CoolEnough(ThermalState())) return;
            var go = coolWaiter;
            coolWaiter = null;
            BeginInvokeOnMainThread(() => { Write("heat: cooled down, starting"); RefreshQueuePage(); go(); });
        });
    }

    /// While a build runs the pipeline reads Documents/gameapp.thermal: "<state> <lowperf>", every 3 s.
    private void StartThermalFile()
    {
        thermalTimer?.Invalidate();
        WriteThermalFile();
        thermalTimer = NSTimer.CreateRepeatingScheduledTimer(3, _ => WriteThermalFile());
    }

    private void WriteThermalFile()
    {
        try { File.WriteAllText(ThermalPath, BuildHeat.ThermalLine(ThermalState(), GameAppUi.LowPerformanceOn(ReadAppSettings()))); }
        catch (Exception) { }
    }

    private void StopThermalFile()
    {
        thermalTimer?.Invalidate();
        thermalTimer = null;
        try { File.Delete(ThermalPath); } catch (Exception) { }
    }
}
