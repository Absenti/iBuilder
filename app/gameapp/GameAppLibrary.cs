// gameapp/GameAppLibrary.cs: the app's screens in the "Ambient" design (picked 2026-11-18, gameapp/GameAppTheme.cs).
//
//   Library  the storage bar (design A), a carousel of the covers (the ground takes the front cover's colour), the
//            selected game's name, line and main button, then every game in a list, the ones only in the cloud
//            greyed with a cloud mark, then the exported apps (.ipa), local and in the cloud.
//   Cloud    the NAS (SFTP, its folder) and folders picked in the Files app (iCloud Drive, Google Drive, OneDrive...):
//            what they hold shows in the library; a tap downloads it, "Remove download" frees the space again.
//   Logs     a timeline: each line with its time, stages as headers, errors and warnings coloured and counted, a
//            switch for errors and warnings only, a jump to the first error.
//   Build    a screen of its own while a game is built (design C): a ring with the percentage, chips, the time left,
//            the memory, and the live log (the last lines; a tap opens the whole log).
//
// What each part shows is decided in GameAppUi.cs and GameAppCloud.cs (tested on a Mac); this file only draws and acts.

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using CoreAnimation;
using CoreGraphics;
using Foundation;
using UIKit;

public sealed partial class GameAppViewController
{
    private readonly AmbientView ambient = new AmbientView();
    private readonly StorageBarView storageBar = new StorageBarView();
    private readonly CoverCarousel carousel = new CoverCarousel();
    private readonly UIStackView appsList = new UIStackView();
    private readonly UILabel runLine = new UILabel();
    private UIButton watchBuild;
    private List<LibraryEntry> entries = new List<LibraryEntry>();
    private UIColor currentTint;

    private sealed record LibraryEntry(string Name, CloudFile Cloud, UIImage Icon);

    private bool IsCloudOnly(string name) => name != null && entries.Any(e => e.Name == name && e.Cloud != null && !File.Exists(Path.Combine(GameAppMain.Documents, name)));

    // ---- Library ------------------------------------------------------------------------------------------------

    private void BuildGamesPage()
    {
        var title = Theme.Label("Library", Theme.Rounded(34, UIFontWeight.Bold), Theme.Text);
        title.AccessibilityTraits = UIAccessibilityTrait.Header;
        var refresh = Theme.Round("arrow.triangle.2.circlepath", "Refresh the cloud");
        refresh.TouchUpInside += (_, __) => RefreshCloud(null);
        var plus = Theme.Round("plus", "Import file");
        plus.AccessibilityIdentifier = "import-file";
        plus.TouchUpInside += (_, __) => Import();
        plus.Hidden = refresh.Hidden = GameExport.IsPlayer;
        var header = Theme.Row(8, title, new UIView(), refresh, plus);

        banner.Axis = UILayoutConstraintAxis.Vertical;
        banner.Spacing = 6;

        carousel.TranslatesAutoresizingMaskIntoConstraints = false;
        carouselHeight = carousel.HeightAnchor.ConstraintEqualTo(250);
        carouselHeight.Active = true;
        carousel.Settled += index =>
        {
            if (index < 0 || index >= entries.Count || entries[index].Name == selected) return;
            selected = entries[index].Name;
            RefreshSelection(GatherFacts());
            RefreshList(GatherFacts());
        };

        detailName.Font = Theme.Rounded(26, UIFontWeight.Bold);
        detailName.TextColor = Theme.Text;
        detailName.TextAlignment = UITextAlignment.Center;
        detailName.Lines = 2;
        detailSummary.Font = Theme.Font(14, UIFontWeight.Regular);
        detailSummary.TextColor = Theme.Muted;
        detailSummary.TextAlignment = UITextAlignment.Center;
        detailSummary.Lines = 0;
        progress.ProgressTintColor = UIColor.White;
        progress.TrackTintColor = UIColor.FromWhiteAlpha(1f, 0.14f);
        progress.Hidden = true;

        action.AccessibilityIdentifier = "primary-action";
        action.TouchUpInside += (_, __) => PrimaryTapped();
        action.HeightAnchor.ConstraintEqualTo(56).Active = true;
        var gearButton = Theme.Round("gearshape.fill", "Game settings", 56);
        gearButton.AccessibilityIdentifier = "game-settings";
        gearButton.TouchUpInside += (_, __) => ShowSettings();
        gearGlass = gearButton;
        var actions = Theme.Row(10, action, gearButton);
        actions.Alignment = UIStackViewAlignment.Fill;
        // Update & DLC, in sight before a build: what the game has, and Add (files, a folder, the cloud).
        contentTitle = Theme.Label("Update & DLC", Theme.Font(15, UIFontWeight.Semibold), Theme.Text);
        contentAdd = new UIButton(UIButtonType.System) { AccessibilityIdentifier = "game-content-add", ShowsMenuAsPrimaryAction = true };
        var ac = UIButtonConfiguration.GrayButtonConfiguration;
        ac.AttributedTitle = new NSAttributedString("Add", new UIStringAttributes { Font = Theme.Font(14, UIFontWeight.Semibold) });
        ac.Image = UIImage.GetSystemImage("plus");
        ac.ImagePadding = 4;
        ac.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
        ac.BaseBackgroundColor = UIColor.FromWhiteAlpha(1f, 0.14f);
        ac.BaseForegroundColor = Theme.Text;
        contentAdd.Configuration = ac;
        contentList.Axis = UILayoutConstraintAxis.Vertical;
        contentList.Spacing = 6;
        var contentCard = Theme.Wrap(Theme.Column(10, Theme.Row(8, contentTitle, new UIView(), contentAdd), contentList), 20, 14);
        contentCard.AccessibilityIdentifier = "game-content";
        contentCardView = contentCard;

        runLine.Font = Theme.Font(13, UIFontWeight.Semibold);
        runLine.TextColor = Theme.Text;
        runLine.Lines = 0;
        watchBuild = Theme.Pill("Watch the build", "list.bullet.rectangle", false, 44);
        watchBuild.TouchUpInside += (_, __) => ShowBuildScreen();
        var runCard = Theme.Wrap(Theme.Column(8, runLine, progress, watchBuild), 20, 14);
        runCard.Hidden = true;
        runCardView = runCard;

        grid.Axis = UILayoutConstraintAxis.Vertical;
        grid.Spacing = 2;
        appsList.Axis = UILayoutConstraintAxis.Vertical;
        appsList.Spacing = 2;
        var listTitle = Theme.Label("All games", Theme.Font(17, UIFontWeight.Semibold), Theme.Text);
        listTitle.AccessibilityTraits = UIAccessibilityTrait.Header;
        appsTitle = Theme.Label("Exported apps", Theme.Font(17, UIFontWeight.Semibold), Theme.Text);
        appsTitle.AccessibilityTraits = UIAccessibilityTrait.Header;
        var panel = Theme.Wrap(Theme.Column(8, listTitle, grid, appsTitle, appsList), 28, 14, Theme.Panel);

        // One column on a phone (and on a narrow iPad window). On a wide iPad the same views are shared between two panes: the sidebar
        // (title, storage, what is missing, all games) and the detail (the cover, its name, the actions, the cards): SetLibraryWide.
        libDetail = new UIView[] { carousel, detailName, detailSummary, actions, contentCardView, runCard };
        libLeft = Theme.Column(14, header, storageBar, banner, carousel, detailName, detailSummary, actions, contentCardView, runCard, panel);
        libRight = Theme.Column(14);
        libRight.LayoutMarginsRelativeArrangement = true;
        ApplyDetailSpacing();
        libScrollL = new UIScrollView { AlwaysBounceVertical = true, ShowsVerticalScrollIndicator = false, ClipsToBounds = false };
        libScrollR = new UIScrollView { AlwaysBounceVertical = true, ShowsVerticalScrollIndicator = false, ClipsToBounds = false, Hidden = true };
        foreach (var (scroll, column, max) in new[] { (libScrollL, libLeft, (nfloat) 700), (libScrollR, libRight, (nfloat) 560) })
        {
            column.TranslatesAutoresizingMaskIntoConstraints = false;
            scroll.TranslatesAutoresizingMaskIntoConstraints = false;
            scroll.AddSubview(column);
            pages[0].AddSubview(scroll);
            Theme.PinColumn(scroll, column, 4, 20, 0, max);
        }
        pages[0].ClipsToBounds = true;
        libCompact = new[]
        {
            libScrollL.TopAnchor.ConstraintEqualTo(pages[0].TopAnchor, 8), libScrollL.BottomAnchor.ConstraintEqualTo(pages[0].BottomAnchor, -8),
            libScrollL.LeadingAnchor.ConstraintEqualTo(pages[0].LeadingAnchor, 18), libScrollL.TrailingAnchor.ConstraintEqualTo(pages[0].TrailingAnchor, -18),
        };
        libSideWidth = libScrollL.WidthAnchor.ConstraintEqualTo(380);
        libWideC = new[]
        {
            libScrollL.TopAnchor.ConstraintEqualTo(pages[0].TopAnchor, 8), libScrollL.BottomAnchor.ConstraintEqualTo(pages[0].BottomAnchor, -8),
            libScrollL.LeadingAnchor.ConstraintEqualTo(pages[0].LeadingAnchor, 18), libSideWidth,
            libScrollR.TopAnchor.ConstraintEqualTo(pages[0].TopAnchor, 8), libScrollR.BottomAnchor.ConstraintEqualTo(pages[0].BottomAnchor, -8),
            libScrollR.LeadingAnchor.ConstraintEqualTo(libScrollL.TrailingAnchor, 24), libScrollR.TrailingAnchor.ConstraintEqualTo(pages[0].TrailingAnchor, -18),
        };
        NSLayoutConstraint.ActivateConstraints(libCompact);
    }

    private UIStackView libLeft, libRight;
    private UIScrollView libScrollL, libScrollR;
    private UIView[] libDetail;
    private NSLayoutConstraint[] libCompact, libWideC;
    private NSLayoutConstraint libSideWidth, carouselHeight;
    private bool libWide;

    private void ApplyDetailSpacing()
    {
        // the gaps of the phone's column, which the detail keeps in either place
        foreach (var column in new[] { libLeft, libRight })
        {
            void Gap(nfloat gap, UIView after) { if (Array.IndexOf(column.ArrangedSubviews, after) >= 0) column.SetCustomSpacing(gap, after); }
            Gap(4, detailName);
            Gap(12, libDetail[3]);
            Gap(18, contentCardView);
        }
    }

    /// The library's two panes (a wide iPad window) or its one column (a phone, a narrow iPad window). The views move between the two
    /// columns; nothing is rebuilt, so the selection, the cover in front and a download's ring stay where they are.
    private void SetLibraryWide(bool wide, nfloat sideWidth, nfloat detailWidth)
    {
        if (libLeft == null) return;
        if (wide != libWide)
        {
            libWide = wide;
            foreach (var v in libDetail)
            {
                (v.Superview as UIStackView)?.RemoveArrangedSubview(v);
                v.RemoveFromSuperview();
            }
            if (wide)
            {
                NSLayoutConstraint.DeactivateConstraints(libCompact);
                NSLayoutConstraint.ActivateConstraints(libWideC);
                foreach (var v in libDetail) libRight.AddArrangedSubview(v);
            }
            else
            {
                NSLayoutConstraint.DeactivateConstraints(libWideC);
                NSLayoutConstraint.ActivateConstraints(libCompact);
                int at = Array.IndexOf(libLeft.ArrangedSubviews, banner) + 1;
                foreach (var v in libDetail) libLeft.InsertArrangedSubview(v, (nuint) at++);
            }
            libScrollR.Hidden = !wide;
            libRight.LayoutMargins = new UIEdgeInsets(wide ? 28 : 0, 0, 0, 0);
            ApplyDetailSpacing();
        }
        if (wide)
        {
            libSideWidth.Constant = sideWidth;
            // the cover in front is larger where there is room for it: a bit over half of the pane, 220 to 300
            var item = (nfloat) Math.Max(220.0, Math.Min(300.0, (double) detailWidth * 0.55));
            carousel.SetItemSize(item);
            carouselHeight.Constant = item + 30;
        }
        else
        {
            carousel.SetItemSize(220);
            carouselHeight.Constant = 250;
        }
    }

    private UIButton gearGlass;
    private UIView runCardView;
    private UILabel appsTitle;

    private static long DumpBytes()
    {
        long n = 0;
        try
        {
            foreach (var f in Directory.EnumerateFiles(GameAppMain.Documents))
                if (Cloud.IsGameFile(f)) n += new FileInfo(f).Length;
        }
        catch (Exception) { }
        return n;
    }

    private static long FreeBytes()
    {
        try { return new DriveInfo("/").AvailableFreeSpace; } catch (Exception) { return 0; }
    }

    private void RefreshGamesPage(UiFacts facts, IReadOnlyList<string> games)
    {
        // The banner: what is missing before anything can run, one tappable row each.
        foreach (var row in banner.ArrangedSubviews.ToArray()) { banner.RemoveArrangedSubview(row); row.RemoveFromSuperview(); }
        foreach (var need in GameAppUi.Prerequisites(facts))
        {
            var importer = need.Importer;
            var view = new BannerView(ChecklistLabel(need.Row), need.Row.Level == CheckLevel.Missing ? UIColor.SystemRed : UIColor.SystemOrange);
            view.AccessibilityLabel = $"{need.Row.Label}: {need.Row.Detail}";
            view.AccessibilityIdentifier = importer == GameAppUi.Importer.Keys ? "banner-keys" : "banner-certificate";
            view.TouchUpInside += (_, __) => { if (importer == GameAppUi.Importer.Keys) Import(); else ImportCertificate(); };
            banner.AddArrangedSubview(view);
        }
        AddKilledBanner();   // GameAppResume.cs: "iOS ended the last build (memory). Resume"
        banner.Hidden = banner.ArrangedSubviews.Length == 0;

        storageBar.Set(CurrentStorageSegments());

        // The entries: the local games, then the ones only in the cloud.
        var localNames = Dumps().Select(Path.GetFileName).ToList();
        var list = games.Select(g => new LibraryEntry(g, Cloud.CloudCopyOf(g, cloudFiles), CoverOf(g, facts))).ToList();
        if (!GameExport.IsPlayer)
            list.AddRange(Cloud.CloudOnlyGames(localNames, cloudFiles).Select(f => new LibraryEntry(f.Name, f, CoverOf(f.Name, facts))));
        entries = list;
        if (selected == null || !entries.Any(e => e.Name == selected)) selected = entries.FirstOrDefault()?.Name;
        carousel.SetItems(entries.Select(e => (e.Icon, GameAppUi.DisplayName(e.Name), IsCloudOnly(e.Name), CoverFraction(e.Name, facts))).ToList());
        carousel.Hidden = entries.Count == 0;
        var index = entries.FindIndex(e => e.Name == selected);
        if (index >= 0 && !carousel.Dragging && !carousel.Decelerating) carousel.Select(index, false);
        RefreshSelection(facts);
        RefreshList(facts);
    }

    /// The ring over a cover: its download, or the build when it is the game being built.
    private double? CoverFraction(string name, UiFacts facts)
    {
        if (downloads.TryGetValue(name, out var d)) return Math.Max(0.02, d);
        if (facts.Running && name == selected && running) return Math.Max(0.02, GameAppUi.ParseStage(lastStage).Fraction);
        return null;
    }

    /// The front cover's name, its line, the main button, and the ground's colour.
    private void RefreshSelection(UiFacts facts)
    {
        var entry = entries.FirstOrDefault(e => e.Name == selected);
        var tint = entry?.Icon != null ? Theme.AverageColor(entry.Icon) : entry != null ? Theme.ColorFor(GameAppUi.DisplayName(entry.Name)) : Theme.DefaultTint;
        if (currentTint == null || !tint.Equals(currentTint)) { currentTint = tint; ambient.SetTint(tint, true); }

        if (entry == null)
        {
            detailName.Text = "No games yet";
            detailSummary.Text = "Tap + to import a .nsp, .xci, .nsz or .nup (Switch 2), or add a cloud folder in Cloud.";
            SetPrimary("Import a game", "plus", true);
            gearGlass.Hidden = true;
            contentCardView.Hidden = true;
            return;
        }
        detailName.Text = GameAppUi.DisplayName(entry.Name);
        if (IsCloudOnly(entry.Name))
        {
            detailSummary.Text = Cloud.Where(entry.Cloud, cloudSources) + $" · not on this {GameAppUi.DeviceName}";
            var busy = downloads.TryGetValue(entry.Name, out var d);
            SetPrimary(busy ? $"Downloading… {Math.Round(d * 100):0}%" : "Download", busy ? null : "icloud.and.arrow.down", !busy && !facts.Running);
            gearGlass.Hidden = true;
            contentCardView.Hidden = true;
            return;
        }
        var names = Dumps().Select(Path.GetFileName).ToList();
        detailSummary.Text = GameAppUi.GameSummary(entry.Name, facts)
            + (entry.Cloud != null ? " · also in " + Cloud.Where(entry.Cloud, cloudSources).Substring(3).Split(" · ")[0] : "")
            + (GameAppUi.ContentChangedSinceBuild(entry.Name, names, facts) ? "\nUpdate or DLC added since the build: rebuild (game settings) to include it" : "");
        var isBase = GameAppUi.ContentKindOf(entry.Name) == GameAppUi.ContentKind.Base && !GameAppUi.IsNup(entry.Name) && !GameExport.IsPlayer;
        contentCardView.Hidden = !isBase;
        if (isBase) FillContentCard(entry.Name, facts);
        var primary = GameAppUi.GameAction(entry.Name, facts);
        SetPrimary(primary.Title, primary.Kind == ActionKind.Boot ? "play.fill" : primary.Kind == ActionKind.Run ? "hammer.fill" : primary.Kind == ActionKind.Export ? "square.and.arrow.up" : null, primary.Enabled);
        gearGlass.Hidden = false;
        gearGlass.Enabled = !facts.Running;
        RefreshRunLine();
    }

    private void SetPrimary(string title, string symbol, bool enabled)
    {
        var c = UIButtonConfiguration.FilledButtonConfiguration;
        c.AttributedTitle = new NSAttributedString(title, new UIStringAttributes { Font = Theme.Font(17, UIFontWeight.Semibold) });
        if (symbol != null) { c.Image = UIImage.GetSystemImage(symbol); c.ImagePadding = 8; }
        c.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
        c.BaseBackgroundColor = UIColor.White;
        c.BaseForegroundColor = Theme.Ground;
        action.Configuration = c;
        action.Enabled = enabled;
    }

    /// The run card under the button: the step it is at and the clock, while a game is built.
    private static string LowerFirst(string s) => string.IsNullOrEmpty(s) ? s : char.ToLowerInvariant(s[0]) + s.Substring(1);

    private void RefreshRunLine()
    {
        if (runCardView == null) return;
        bool shown = running || (startedAt != default && lastRunGame != null && lastRunGame == selected);
        runCardView.Hidden = !shown;
        if (!shown) return;
        var wc = watchBuild.Configuration;
        wc.AttributedTitle = new NSAttributedString(running ? "Watch the build" : "See the build", new UIStringAttributes { Font = Theme.Font(16, UIFontWeight.Semibold) });
        watchBuild.Configuration = wc;
        if (!running)
        {
            var at = GameAppUi.BuildSteps(lastStage, stepStarts, lastRunEnd).FirstOrDefault(r => r.State == GameAppUi.StepState.Now);
            runLine.Text = failedRun ? $"Stopped at {LowerFirst(at?.Name ?? "the start")} · {GameAppUi.FormatElapsed(lastRunEnd - startedAt)}"
                                     : $"Built in {GameAppUi.FormatElapsed(lastRunEnd - startedAt)}";
            runLine.TextColor = failedRun ? Theme.Warn : Theme.Ok;
            progress.Hidden = true;
            return;
        }
        runLine.TextColor = Theme.Text;
        progress.Hidden = false;
        var s = GameAppUi.ParseStage(lastStage);
        var step = GameAppUi.BuildSteps(lastStage, stepStarts, DateTime.Now).FirstOrDefault(r => r.State == GameAppUi.StepState.Now);
        runLine.Text = $"{Math.Round(s.Fraction * 100):0}% · {step?.Name ?? "Starting"}{(string.IsNullOrEmpty(step?.Detail) ? "" : " · " + step.Detail)} · {GameAppUi.FormatElapsed(DateTime.Now - startedAt)}";
    }

    private void RefreshList(UiFacts facts)
    {
        foreach (var v in grid.ArrangedSubviews.ToArray()) { grid.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        if (entries.Count == 0)
            grid.AddArrangedSubview(Theme.Label("Nothing here yet.", Theme.Font(14, UIFontWeight.Regular), Theme.Muted));
        for (int i = 0; i < entries.Count; i++)
        {
            var e = entries[i];
            int index = i;
            bool cloudOnly = IsCloudOnly(e.Name);
            string sub, title = null, symbol = null;
            bool primary = false;
            if (downloads.TryGetValue(e.Name, out var d)) { sub = $"Downloading… {Math.Round(d * 100):0}%"; }
            else if (cloudOnly) { sub = Cloud.Where(e.Cloud, cloudSources); symbol = "icloud.and.arrow.down"; }
            else
            {
                var ready = GameAppUi.GameStatusOf(e.Name, facts) == GameAppUi.GameStatus.Ready;
                sub = (ready ? "Ready" : "Not built yet") + (e.Cloud != null ? " · also in the cloud" : "");
                if (!facts.Running) { title = ready ? "Play" : "Build"; primary = ready; symbol = ready ? "play.fill" : null; }
            }
            var row = new GameRowView(e.Icon, GameAppUi.DisplayName(e.Name), sub, cloudOnly, e.Name == selected, title, symbol, primary, MenuFor(e, cloudOnly));
            row.AccessibilityIdentifier = "game-row-" + i;
            row.TouchUpInside += (_, __) => { selected = e.Name; carousel.Select(index, true); RefreshSelection(GatherFacts()); RefreshList(GatherFacts()); };
            row.Trailing.TouchUpInside += (_, __) =>
            {
                selected = e.Name;
                if (cloudOnly) DownloadSelected(); else PrimaryTapped();
            };
            grid.AddArrangedSubview(row);
        }

        // Exported apps: local, then the cloud's.
        int appCount = FillApps(appsList);
        appsTitle.Hidden = appsList.Hidden = appCount == 0 || GameExport.IsPlayer;
        if (currentTab == 5) RefreshAppsPage();
    }

    /// The exported apps (.ipa) as rows in `list`: those on this iPhone (Share, Upload to…, Delete), then the cloud's
    /// (Download). The library's "Exported apps" and Settings › Exported apps show the same rows. Returns how many.
    private int FillApps(UIStackView list)
    {
        foreach (var v in list.ArrangedSubviews.ToArray()) { list.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        var dir = Path.Combine(GameAppMain.Documents, "Exports");
        var local = Directory.Exists(dir) ? Directory.GetFiles(dir, "*.ipa").Select(Path.GetFileName).OrderBy(n => n).ToList() : new List<string>();
        var apps = Cloud.Apps(local, cloudFiles);
        foreach (var (name, isLocal, cloud) in apps)
        {
            var path = Path.Combine(dir, name);
            // A slim export's game files, beside it (GameAppUi.RomfsFileName): shown with it, deleted with it.
            var romfs = isLocal ? GameAppUi.FindRomfs(dir, Path.GetFileNameWithoutExtension(name)) : null;
            string sub = downloads.TryGetValue(name, out var d) ? $"Downloading… {Math.Round(d * 100):0}%"
                       : isLocal ? GameAppUi.SizeText(new FileInfo(path).Length)
                                   + (romfs != null ? $" · Light, game files {GameAppUi.SizeText(new FileInfo(romfs).Length)} beside it" : " · Full")
                                   + (cloud != null ? " · also in the cloud" : "")
                       : Cloud.Where(cloud, cloudSources);
            var menu = new List<UIMenuElement>();
            GameRowView row = null;
            if (isLocal)
            {
                menu.Add(UIAction.Create("Share…", UIImage.GetSystemImage("square.and.arrow.up"), null, _ => ShareFile(path, row)));
                foreach (var target in UploadTargets())
                    menu.Add(UIAction.Create("Upload to " + target.Name, UIImage.GetSystemImage("icloud.and.arrow.up"), null, _ => UploadTo(target, path, true)));
                if (romfs != null)
                    menu.Add(UIAction.Create("Show the game files in Files", UIImage.GetSystemImage("folder"), null, _ => ShowInFiles(romfs)));
                menu.Add(UIAction.Create(cloud != null ? "Remove download (stays in the cloud)" : romfs != null ? "Delete (and its game files)" : "Delete",
                                         UIImage.GetSystemImage("trash"), null,
                                         _ => RemoveFiles(romfs != null ? new[] { path, romfs } : new[] { path }, cloud != null)));
            }
            row = new GameRowView(null, Path.GetFileNameWithoutExtension(name), sub, !isLocal, false,
                                      isLocal ? "" : null, isLocal ? "square.and.arrow.up" : downloads.ContainsKey(name) ? null : "icloud.and.arrow.down", false,
                                      menu.Count > 0 ? UIMenu.Create(menu.ToArray()) : null);
            row.Trailing.TouchUpInside += (_, __) =>
            {
                if (isLocal) ShareFile(path, row.Trailing);
                else if (!downloads.ContainsKey(name)) Download(new List<CloudFile> { cloud }, dir);
            };
            if (isLocal) row.Trailing.AccessibilityLabel = "Share " + Path.GetFileNameWithoutExtension(name);   // an icon alone: the name keeps the room
            row.AccessibilityIdentifier = "app-row-" + list.ArrangedSubviews.Length;
            list.AddArrangedSubview(row);
        }
        return apps.Count;
    }

    // ---- Settings › Exported apps ----------------------------------------------------------------------------------

    private readonly UIStackView appsPageList = new UIStackView();
    private UIView appsPageCard;
    private UILabel appsPageNote, appsDetail;

    private void BuildAppsPage()
    {
        appsPageList.Axis = UILayoutConstraintAxis.Vertical;
        appsPageList.Spacing = 2;
        var header = SubHeader("Exported apps");
        appsPageNote = Theme.Label("", Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0);
        appsPageCard = Theme.Wrap(appsPageList, 22, 10);
        var column = Theme.Column(14, header, appsPageNote, appsPageCard);
        var scroll = new UIScrollView { AlwaysBounceVertical = true };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        Theme.PinColumn(scroll, column, 4, 20);
        Fill(pages[5], scroll, 18);
    }

    private void RefreshAppsPage()
    {
        if (appsPageCard == null) return;
        int n = FillApps(appsPageList);
        appsPageCard.Hidden = n == 0;
        appsPageNote.Text = n == 0
            ? "No exported app yet. A built game's settings › Export as .ipa makes one: it shows here and in Files › iBuilder › Exports."
            : "Share one to SideStore or FlareStore to install it, or upload it to the cloud (the ··· menu). They are also in Files › iBuilder › Exports.";
    }

    /// Settings' line for the exported apps: how many, and their size.
    private static string AppsSummary()
    {
        var dir = Path.Combine(GameAppMain.Documents, "Exports");
        var files = Directory.Exists(dir) ? Directory.GetFiles(dir, "*.ipa") : Array.Empty<string>();
        if (files.Length == 0) return "None";
        // A Light IPA's game files beside it count with it: the space the exports take.
        long bytes = Directory.GetFiles(dir, "*.ipa").Concat(GameAppUi.AllRomfs(dir)).Sum(f => new FileInfo(f).Length);
        return $"{files.Length} · {GameAppUi.SizeText(bytes)}";
    }

    private UIMenu MenuFor(LibraryEntry e, bool cloudOnly)
    {
        var items = new List<UIMenuElement>();
        var path = Path.Combine(GameAppMain.Documents, e.Name);
        if (cloudOnly)
            items.Add(UIAction.Create("Download", UIImage.GetSystemImage("icloud.and.arrow.down"), null, _ => { selected = e.Name; DownloadSelected(); }));
        else
        {
            if (e.Cloud != null)
                items.Add(UIAction.Create("Remove download (stays in the cloud)", UIImage.GetSystemImage("icloud.slash"), null, _ => RemoveGameDownload(e.Name)));
            foreach (var target in UploadTargets())
                if (e.Cloud == null || e.Cloud.SourceId != target.Id)
                    items.Add(UIAction.Create("Upload to " + target.Name, UIImage.GetSystemImage("icloud.and.arrow.up"), null, _ => UploadTo(target, path, false)));
            items.Add(UIAction.Create("Add to the build queue", UIImage.GetSystemImage("list.number"), null, _ => AddToBuildQueue(e.Name)));
            items.Add(UIAction.Create("Settings", UIImage.GetSystemImage("gearshape"), null, _ => { selected = e.Name; ShowSettings(); }));
        }
        return items.Count == 0 ? null : UIMenu.Create(items.ToArray());
    }

    /// Opens the Files app on the folder holding `path` (shareddocuments: the app's own folder in Files).
    private static void ShowInFiles(string path)
    {
        var url = new NSUrl("shareddocuments://" + Uri.EscapeUriString(Path.GetDirectoryName(path)));
        UIApplication.SharedApplication.OpenUrl(url, new UIApplicationOpenUrlOptions(), null);
    }

    private void ShareFile(string path, UIView from)
    {
        var activity = new UIActivityViewController(new NSObject[] { NSUrl.FromFilename(path) }, null);
        Theme.Anchor(activity, from, View);
        PresentWhenFree(activity);
    }

    // ---- an exported game (.ipa): its title screen (design F1) -------------------------------------------------
    //
    // The game's own icon large on the ground lit by its colour, its name and when it was last saved, one Play, then
    // Saves and Settings with what they hold in a line, and "Made with iBuilder". No tabs, no build, no cloud.

    private UILabel playerSaves, playerSettings, playerLast, playerLogs;

    private UIView logsTile;

    /// An exported game's logs: ONE zip, "<Game> logs <date>.zip", with every log file of this app as far back as
    /// 10 minutes, complete (GameAppLogs.cs), through the share sheet.
    private void ShareGameLogs(string game, UIView source)
    {
        var name = GameAppUi.DisplayName(game);
        System.Threading.Tasks.Task.Run(() =>
        {
            string zip = null, failure = null;
            try { zip = LogWindow.BuildZip(GameAppMain.Documents, Path.Combine(Path.GetTempPath(), "logs-share"), name, DateTime.Now); }
            catch (Exception e) { failure = e.Message; }
            BeginInvokeOnMainThread(() =>
            {
                if (zip == null)
                {
                    var a = UIAlertController.Create("Logs", failure == null ? "There are no logs yet." : "The logs could not be packed: " + failure, UIAlertControllerStyle.Alert);
                    a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                    PresentWhenFree(a);
                    return;
                }
                Write($"logs: sharing {Path.GetFileName(zip)} ({GameAppUi.SizeText(new FileInfo(zip).Length)})");
                var share = new UIActivityViewController(new NSObject[] { NSUrl.FromFilename(zip) }, null);
                Theme.Anchor(share, source, View);
                PresentWhenFree(share);
            });
        });
    }

    // ---- launch: the first frame first --------------------------------------------------------------------------

    private readonly System.Diagnostics.Stopwatch launchClock = System.Diagnostics.Stopwatch.StartNew();
    private bool firstFrameDone;
    private NSTimer logMarkTimer;

    /// The library is on screen: say how long that took, then start what the first screen does not need (the cloud
    /// listing and its icons, after a short pause so the first frame is drawn and touchable), and the log marks.
    public override void ViewDidAppear(bool animated)
    {
        base.ViewDidAppear(animated);
        if (firstFrameDone) return;
        firstFrameDone = true;
        Write($"launch: first screen in {launchClock.ElapsedMilliseconds} ms");
        NSTimer.CreateScheduledTimer(0.8, _ =>
        {
            Write($"launch: cloud listing starts at {launchClock.ElapsedMilliseconds} ms");
            RefreshCloud(null);
            FetchCloudIcons();
            QueueResumeIfOwed();
        });
        // Marks of the log files' lengths every 30 s: what a log share needs to hold the last 10 minutes, complete.
        var documents = GameAppMain.Documents;
        System.Threading.Tasks.Task.Run(() => LogWindow.Mark(documents, DateTime.UtcNow));
        logMarkTimer = NSTimer.CreateRepeatingScheduledTimer(LogWindow.MarkEvery.TotalSeconds,
            _ => System.Threading.Tasks.Task.Run(() => LogWindow.Mark(documents, DateTime.UtcNow)));
    }

    private void BuildPlayerPage()
    {
        var game = GameExport.PlayerDump;
        selected = game;
        var facts = GatherFacts();
        var icon = CoverOf(game, facts);
        var tint = icon != null ? Theme.AverageColor(icon) : Theme.ColorFor(GameAppUi.DisplayName(game));
        currentTint = tint;
        ambient.SetTint(tint, false);

        var cover = new CoverView();
        cover.Set(icon, GameAppUi.DisplayName(game), false, null);
        cover.TranslatesAutoresizingMaskIntoConstraints = false;
        cover.WidthAnchor.ConstraintEqualTo(220).Active = true;
        cover.HeightAnchor.ConstraintEqualTo(220).Active = true;
        cover.Layer.ShadowColor = UIColor.Black.CGColor;
        cover.Layer.ShadowOpacity = 0.5f;
        cover.Layer.ShadowRadius = 30;
        cover.Layer.ShadowOffset = new CGSize(0, 20);
        var coverRow = Theme.Row(0, new UIView(), cover, new UIView());
        coverRow.Distribution = UIStackViewDistribution.EqualCentering;

        var title = Theme.Label(GameAppUi.DisplayName(game), Theme.Rounded(28, UIFontWeight.Bold), Theme.Text, 3);
        title.TextAlignment = UITextAlignment.Center;
        title.AccessibilityTraits = UIAccessibilityTrait.Header;
        playerLast = Theme.Label("", Theme.Font(14, UIFontWeight.Regular), Theme.Hex(0xC3C9E2), 2);
        playerLast.TextAlignment = UITextAlignment.Center;

        playerPlay = new UIButton(UIButtonType.System) { AccessibilityIdentifier = "primary-action" };
        playerPlay.HeightAnchor.ConstraintEqualTo(64).Active = true;
        playerPlay.TouchUpInside += (_, __) => { if (PlayerNeedsGameFiles) FindGameFiles(); else PrimaryTapped(); };
        playerStatus = Theme.Label("", Theme.Font(13, UIFontWeight.Medium), Theme.Muted, 0);
        playerStatus.TextAlignment = UITextAlignment.Center;

        UIButton Tile(string name, out UILabel line, string id, Action tapped)
        {
            var b = new UIButton(UIButtonType.System) { AccessibilityIdentifier = id };
            b.BackgroundColor = Theme.Glass;
            b.Layer.CornerRadius = 22;
            b.Layer.BorderWidth = 1;
            b.Layer.BorderColor = Theme.GlassLine.CGColor;
            var head = Theme.Label(name, Theme.Font(16, UIFontWeight.Semibold), Theme.Text);
            line = Theme.Label("", Theme.Font(12, UIFontWeight.Regular), Theme.Muted, 2);
            var col = Theme.Column(3, head, line);
            col.UserInteractionEnabled = false;
            col.TranslatesAutoresizingMaskIntoConstraints = false;
            b.AddSubview(col);
            NSLayoutConstraint.ActivateConstraints(new[]
            {
                col.LeadingAnchor.ConstraintEqualTo(b.LeadingAnchor, 16), col.TrailingAnchor.ConstraintEqualTo(b.TrailingAnchor, -12),
                col.CenterYAnchor.ConstraintEqualTo(b.CenterYAnchor), b.HeightAnchor.ConstraintEqualTo(80),
            });
            b.AccessibilityLabel = name;
            b.TouchUpInside += (_, __) => tapped();
            return b;
        }
        var saves = Tile("Saves", out playerSaves, "player-saves", () => ShowSaves(game));
        var settings = Tile("Settings", out playerSettings, "player-settings", () => { selected = game; ShowSettings(); });
        var logs = Tile("Logs", out playerLogs, "player-logs", () => ShareGameLogs(game, logsTile));
        playerLogs.Text = "last 10 minutes, one zip";
        logsTile = logs;
        var tiles = new UIStackView(new UIView[] { saves, settings, logs }) { Axis = UILayoutConstraintAxis.Horizontal, Distribution = UIStackViewDistribution.FillEqually, Spacing = 10 };

        var footer = Theme.Label("Made with iBuilder", Theme.Font(12, UIFontWeight.Regular), Theme.Dim);
        footer.TextAlignment = UITextAlignment.Center;

        var column = Theme.Column(18, coverRow, title, playerLast, playerPlay, playerStatus, tiles);
        column.SetCustomSpacing(6, title);
        column.SetCustomSpacing(26, playerLast);
        column.SetCustomSpacing(8, playerPlay);
        var scroll = new UIScrollView { AlwaysBounceVertical = true, ShowsVerticalScrollIndicator = false, ClipsToBounds = false };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        footer.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        View.AddSubview(scroll);
        View.AddSubview(footer);
        var g = View.SafeAreaLayoutGuide;
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            scroll.TopAnchor.ConstraintEqualTo(g.TopAnchor), scroll.LeadingAnchor.ConstraintEqualTo(g.LeadingAnchor),
            scroll.TrailingAnchor.ConstraintEqualTo(g.TrailingAnchor), scroll.BottomAnchor.ConstraintEqualTo(footer.TopAnchor, -8),
            footer.LeadingAnchor.ConstraintEqualTo(g.LeadingAnchor), footer.TrailingAnchor.ConstraintEqualTo(g.TrailingAnchor),
            footer.BottomAnchor.ConstraintEqualTo(g.BottomAnchor, -8),
        });
        Theme.PinColumn(scroll, column, 48, 16, 24, 520);   // an iPad keeps the title screen a phone's width
        RefreshPlayer(facts);
    }

    /// A slim export not yet shown its game files (GameExport.PlayerRomfs).
    private static bool PlayerNeedsGameFiles => GameExport.PlayerRomfsName != null && GameExport.PlayerRomfs == null;

    private FolderPickerDelegate gameFilesPicker;

    /// Files, filtered to game files (.romfs, declared in Info.plist): the player picks its game's, opened in place.
    private void FindGameFiles()
    {
        var picker = new UIDocumentPickerViewController(new[] { "dev.nxpocket.ibuilder.romfs" }, UIDocumentPickerMode.Open);
        gameFilesPicker = new FolderPickerDelegate(url =>
        {
            if (url == null) return;
            var why = GameExport.UsePickedRomfs(url);
            Write(why == null ? $"game files: {url.LastPathComponent}, read in place from now on" : $"game files refused: {why}");
            if (why != null)
            {
                var a = UIAlertController.Create("Not this game's files", why, UIAlertControllerStyle.Alert);
                a.AddAction(UIAlertAction.Create("Pick again", UIAlertActionStyle.Default, _ => FindGameFiles()));
                a.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
                PresentWhenFree(a);
            }
            RefreshScreen();
        });
        picker.Delegate = gameFilesPicker;
        PresentWhenFree(picker);
    }

    private void RefreshPlayer(UiFacts facts)
    {
        if (playerPlay == null) return;
        var game = GameExport.PlayerDump;
        var next = GameAppUi.GameAction(game, facts);
        bool needFiles = PlayerNeedsGameFiles;
        var c = UIButtonConfiguration.FilledButtonConfiguration;
        c.AttributedTitle = new NSAttributedString(needFiles ? "Find the game files" : next.Kind == ActionKind.Boot ? "Play" : next.Title,
                                                   new UIStringAttributes { Font = Theme.Font(20, UIFontWeight.Bold) });
        c.Image = needFiles ? UIImage.GetSystemImage("folder.fill") : next.Kind == ActionKind.Boot ? UIImage.GetSystemImage("play.fill") : null;
        c.ImagePadding = 10;
        c.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
        c.BaseBackgroundColor = UIColor.White;
        c.BaseForegroundColor = Theme.Ground;
        playerPlay.Configuration = c;
        playerPlay.Enabled = needFiles || next.Enabled;
        playerStatus.Text = facts.Running ? "Starting the game…"
            : needFiles ? $"Once: in On My iPhone › iBuilder › Exports › {GameAppUi.ExportFileName(GameAppUi.DisplayName(game))}, pick “{GameExport.PlayerRomfsName}” "
                          + $"(or search {GameAppUi.BaseTitleIdOf(game)}). The game is read from there: keep that file and iBuilder."
            : "";
        playerStatus.Hidden = playerStatus.Text.Length == 0;

        // When it was last saved, how many copies are kept, how the game is set.
        var tid = GameAppUi.BaseTitleIdOf(game);
        var docs = GameAppMain.Documents;
        if (tid != null)
        {
            var (bytes, changed) = GameSaves.Current(docs, tid);
            playerLast.Text = changed == null ? $"No save yet: your progress is kept on this {GameAppUi.DeviceName}" : "Last saved " + Ago(changed.Value);
            var copies = GameSaves.List(docs, tid, GameAppUi.DisplayName(game));
            playerSaves.Text = copies.Count == 0 ? "no copy yet" : $"copy {Ago(copies[0].When)} · {copies.Count} kept";
        }
        var v = LoadSettingValues(game);
        string Get(string k) => v.TryGetValue(k, out var x) ? x : null;
        var mode = Get("DM_OPMODE") switch { "1" => "Docked", "0" => "Handheld", _ => "Auto mode" };
        var picture = Get("DM_PICTURE_FIT") == "fill" ? "Fill" : "Fit";
        var fps = Get("DM_FPS_TARGET") is string f ? f + " fps" : "game's fps";
        playerSettings.Text = $"{mode} · {picture} · {fps}";
    }

    /// "2 h ago", "yesterday", "3 days ago", "12 Nov".
    private static string Ago(DateTime when)
    {
        var d = DateTime.Now - when;
        if (d.TotalMinutes < 1) return "just now";
        if (d.TotalHours < 1) return $"{(int) d.TotalMinutes} min ago";
        if (d.TotalHours < 24 && when.Date == DateTime.Today) return $"{(int) d.TotalHours} h ago";
        if (when.Date == DateTime.Today.AddDays(-1)) return "yesterday";
        if (d.TotalDays < 7) return $"{(int) d.TotalDays} days ago";
        return when.ToString("d MMM", System.Globalization.CultureInfo.InvariantCulture);
    }

    // ---- the keyboard (an iPad with a hardware keyboard) -------------------------------------------------------------

    /// Command-R refreshes the cloud, Command-I imports, Command-1 and Command-2 are the tabs, Command-comma is Settings, Return and
    /// Space press the selected game's main button. Held down, Command shows the first five in the system's list. They act on the
    /// library's screen only: a sheet or an alert on top has its own keys.
    private void AddKeyCommands()
    {
        void Add(string key, UIKeyModifierFlags mods, string selector, string title)
        {
            var command = UIKeyCommand.Create(new NSString(key), mods, new ObjCRuntime.Selector(selector));
            if (title != null) command.DiscoverabilityTitle = new NSString(title);
            AddKeyCommand(command);
        }
        Add("r", UIKeyModifierFlags.Command, "keyRefresh", "Refresh the cloud");
        Add("i", UIKeyModifierFlags.Command, "keyImport", "Import a file");
        Add("1", UIKeyModifierFlags.Command, "keyLibrary", "Library");
        Add("2", UIKeyModifierFlags.Command, "keySettingsTab", "Settings");
        Add(",", UIKeyModifierFlags.Command, "keySettingsTab", null);   // the system's own shortcut for an app's settings
        Add("\r", 0, "keyPrimary", "Play or build the selected game");
        Add(" ", 0, "keyPrimary", null);
    }

    private bool KeysAreFree => PresentedViewController == null && !GameExport.IsPlayer;

    [Export("keyRefresh")] private void KeyRefresh() { if (KeysAreFree) RefreshCloud(null); }
    [Export("keyImport")] private void KeyImport() { if (KeysAreFree && !running) Import(); }
    [Export("keyLibrary")] private void KeyLibrary() { if (KeysAreFree) { logsBack = null; ShowTab(0); } }
    [Export("keySettingsTab")] private void KeySettingsTab() { if (KeysAreFree) { logsBack = null; ShowTab(4); } }
    [Export("keyPrimary")] private void KeyPrimary() { if (KeysAreFree && currentTab == 0 && action.Enabled) PrimaryTapped(); }

    // ---- the tab bar: a floating glass capsule ----------------------------------------------------------------

    private void BuildTabBar()
    {
        var bar = new UIVisualEffectView(UIBlurEffect.FromStyle(UIBlurEffectStyle.SystemUltraThinMaterialDark));
        bar.Layer.CornerRadius = 32;
        bar.ClipsToBounds = true;
        bar.Layer.BorderWidth = 1;
        bar.Layer.BorderColor = Theme.GlassLine.CGColor;
        var stack = new UIStackView(tabs) { Axis = UILayoutConstraintAxis.Horizontal, Distribution = UIStackViewDistribution.FillEqually, Spacing = 4 };
        for (int i = 0; i < tabs.Length; i++)
        {
            int index = i;
            tabs[i].AccessibilityIdentifier = TabIds[i];
            tabs[i].AccessibilityLabel = TabTitles[i];
            tabs[i].TouchUpInside += (_, __) => { logsBack = null; ShowTab(index == 0 ? 0 : 4); };
        }
        foreach (var view in new UIView[] { bar, stack }) view.TranslatesAutoresizingMaskIntoConstraints = false;
        foreach (var page in pages)
        {
            page.TranslatesAutoresizingMaskIntoConstraints = false;
            View.AddSubview(page);
        }
        View.AddSubview(bar);
        bar.ContentView.AddSubview(stack);
        var guide = View.SafeAreaLayoutGuide;
        // The capsule is as wide as the screen less 16 pt each side on a phone; on an iPad it keeps a width of its own, centred.
        var barWidth = bar.WidthAnchor.ConstraintEqualTo(guide.WidthAnchor, 1, -32);
        barWidth.Priority = 999;
        var constraints = new List<NSLayoutConstraint>
        {
            bar.CenterXAnchor.ConstraintEqualTo(guide.CenterXAnchor),
            bar.WidthAnchor.ConstraintLessThanOrEqualTo(420),
            barWidth,
            bar.BottomAnchor.ConstraintEqualTo(guide.BottomAnchor, -4),
            bar.HeightAnchor.ConstraintEqualTo(64),
            stack.TopAnchor.ConstraintEqualTo(bar.ContentView.TopAnchor, 6),
            stack.BottomAnchor.ConstraintEqualTo(bar.ContentView.BottomAnchor, -6),
            stack.LeadingAnchor.ConstraintEqualTo(bar.ContentView.LeadingAnchor, 6),
            stack.TrailingAnchor.ConstraintEqualTo(bar.ContentView.TrailingAnchor, -6),
        };
        for (int i = 0; i < pages.Length; i++)
        {
            var page = pages[i];
            pageLead[i] = page.LeadingAnchor.ConstraintEqualTo(guide.LeadingAnchor);
            pageTrail[i] = page.TrailingAnchor.ConstraintEqualTo(guide.TrailingAnchor);
            constraints.Add(page.TopAnchor.ConstraintEqualTo(guide.TopAnchor));
            constraints.Add(pageLead[i]);
            constraints.Add(pageTrail[i]);
            constraints.Add(page.BottomAnchor.ConstraintEqualTo(bar.TopAnchor, -6));
        }
        // The Settings split (a wide iPad window): Settings keeps a column of its own on the left, its pages open on the right.
        pageLeftWidth = pages[4].WidthAnchor.ConstraintEqualTo(380);
        splitDivider = new UIView { BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.08f), Hidden = true, UserInteractionEnabled = false, TranslatesAutoresizingMaskIntoConstraints = false };
        View.AddSubview(splitDivider);
        splitDividerLead = splitDivider.LeadingAnchor.ConstraintEqualTo(guide.LeadingAnchor, 380);
        constraints.Add(splitDividerLead);
        constraints.Add(splitDivider.WidthAnchor.ConstraintEqualTo(1));
        constraints.Add(splitDivider.TopAnchor.ConstraintEqualTo(guide.TopAnchor, 14));
        constraints.Add(splitDivider.BottomAnchor.ConstraintEqualTo(bar.TopAnchor, -14));
        NSLayoutConstraint.ActivateConstraints(constraints.ToArray());
    }

    private readonly NSLayoutConstraint[] pageLead = new NSLayoutConstraint[7], pageTrail = new NSLayoutConstraint[7];
    private NSLayoutConstraint pageLeftWidth, splitDividerLead;
    private UIView splitDivider;
    private bool splitMode;          // a wide iPad window: Settings on the left, the page it opens on the right
    private int splitDetail = 1;     // the page on the right while Settings is the tab: the last one opened (Cloud at first)
    private readonly List<UIButton> subBacks = new List<UIButton>();   // the sub-pages' back buttons: not needed beside Settings
    private readonly Dictionary<int, UIButton> navRows = new Dictionary<int, UIButton>();   // Settings' rows by the page they open

    /// The iPad's wide layout is one decision about the width the window has: two panes in the library, a split in Settings. A phone
    /// and a narrow iPad window (Split View, Slide Over, a third of Stage Manager) keep the one-column layout with its tab capsule.
    /// Called after every layout: a rotation or a resize changes the width, nothing else does.
    private void ApplyAdaptiveLayout()
    {
        if (GameExport.IsPlayer || pageLead[0] == null) return;
        var width = View.SafeAreaLayoutGuide.LayoutFrame.Width;
        bool wide = width >= WideFrom;
        var side = (nfloat) Math.Max(340.0, Math.Min(420.0, (double) width * 0.34));
        SetLibraryWide(wide, side, width - 18 - side - 24 - 18);
        if (wide != splitMode)
        {
            splitMode = wide;
            foreach (var back in subBacks) back.Hidden = wide;
            // Settings (4) is the left column, the five sub-pages share the right one
            pageTrail[4].Active = !wide;
            pageLeftWidth.Active = wide;
            foreach (var i in new[] { 1, 2, 3, 5, 6 }) pageLead[i].Active = true;
            splitDivider.Hidden = !(wide && currentTab != 0);
            ShowTab(currentTab);
        }
        if (wide)
        {
            pageLeftWidth.Constant = side;
            foreach (var i in new[] { 1, 2, 3, 5, 6 }) pageLead[i].Constant = side;
            splitDividerLead.Constant = side;
        }
        else
        {
            foreach (var i in new[] { 1, 2, 3, 5, 6 }) pageLead[i].Constant = 0;
        }
    }

    /// The window is wide enough for the two-pane library and the Settings split: an iPad's full width, an iPad in a wide Stage Manager
    /// window, not a Split View half and never an iPhone (the library is upright there, and 393 pt).
    private const double WideFrom = 820;

    private void ShowTab(int index)
    {
        // A sub-page (Cloud, Keys, Logs) goes back where it was opened from: Settings, or the library (an export's
        // log, the upload's cloud page, a banner); moving between sub-pages keeps the first one's way back.
        if (index != 0 && index != 4 && (currentTab == 0 || currentTab == 4)) subPageReturn = currentTab;
        // Beside the Settings column the tab Settings shows the page that was last open on the right
        if (splitMode && index == 4) index = splitDetail;
        if (index != 0 && index != 4) splitDetail = index;
        currentTab = index;
        bool settingsBeside = splitMode && index != 0;
        for (int i = 0; i < pages.Length; i++) pages[i].Hidden = i != index && !(settingsBeside && i == 4);
        if (splitDivider != null) splitDivider.Hidden = !settingsBeside;
        foreach (var (page, row) in navRows)
            row.BackgroundColor = settingsBeside && page == index ? UIColor.FromWhiteAlpha(1f, 0.09f) : UIColor.Clear;
        int lit = index == 0 ? 0 : 1;   // Cloud, Keys and Logs are Settings' pages
        for (int i = 0; i < tabs.Length; i++)
        {
            bool on = i == lit;
            var config = on ? UIButtonConfiguration.FilledButtonConfiguration : UIButtonConfiguration.PlainButtonConfiguration;
            config.Image = UIImage.GetSystemImage(TabIcons[i], UIImageSymbolConfiguration.Create(UIFont.SystemFontOfSize(15, UIFontWeight.Semibold)));
            config.ImagePadding = 8;
            config.AttributedTitle = new NSAttributedString(TabTitles[i], new UIStringAttributes { Font = Theme.Font(15, on ? UIFontWeight.Bold : UIFontWeight.Semibold) });
            config.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
            config.BaseBackgroundColor = UIColor.White;
            config.BaseForegroundColor = on ? Theme.Ground : Theme.Muted;
            tabs[i].Configuration = config;
            tabs[i].AccessibilityTraits = on ? UIAccessibilityTrait.Button | UIAccessibilityTrait.Selected : UIAccessibilityTrait.Button;
        }
        if (index == 1) RefreshCloudPage();
        if (index == 3) ScrollLogToEnd();
        if (index == 4 || settingsBeside) RefreshSettingsPage(GatherFacts());
        if (index == 5) RefreshAppsPage();
        if (index == 6) RefreshQueuePage();
    }

    /// Where the log's back button goes when the build screen opened it: that screen again. Cleared once used, and by
    /// the tab bar.
    private Action logsBack;
    private int subPageReturn = 4;

    /// A sub-page's header: back where the page was opened from, and its title.
    private UIView SubHeader(string title, params UIView[] trailing)
    {
        var back = Theme.Round("chevron.left", "Back");
        back.Hidden = splitMode;
        subBacks.Add(back);
        back.TouchUpInside += (_, __) =>
        {
            var to = logsBack;
            logsBack = null;
            if (to != null && currentTab == 3) { ShowTab(0); to(); }
            else ShowTab(subPageReturn);
        };
        var label = Theme.Label(title, Theme.Rounded(28, UIFontWeight.Bold), Theme.Text);
        label.AccessibilityTraits = UIAccessibilityTrait.Header;
        label.AdjustsFontSizeToFitWidth = true;
        label.MinimumScaleFactor = 0.7f;
        var views = new List<UIView> { back, label, new UIView() };
        views.AddRange(trailing);
        return Theme.Row(10, views.ToArray());
    }

    // ---- Settings (design D1): a one-line status, then Cloud, Keys and Logs, the build's settings, this iPhone ---------

    private readonly UIButton statusBanner = new UIButton(UIButtonType.System);
    private readonly UILabel statusTitle = new UILabel(), statusDetail = new UILabel();
    private readonly UIImageView statusMark = new UIImageView();
    private UILabel cloudDetail, keysDetail, logsBadge;
    private readonly StorageBarView settingsStorage = new StorageBarView();
    private Action statusFix;

    private void BuildSettingsPage()
    {
        var title = Theme.Label("Settings", Theme.Rounded(34, UIFontWeight.Bold), Theme.Text);
        title.AccessibilityTraits = UIAccessibilityTrait.Header;

        statusTitle.Font = Theme.Font(16, UIFontWeight.Semibold);
        statusTitle.TextColor = Theme.Text;
        statusTitle.Lines = 0;
        statusDetail.Font = Theme.Font(12, UIFontWeight.Regular);
        statusDetail.Lines = 0;
        statusMark.ContentMode = UIViewContentMode.Center;
        statusMark.TranslatesAutoresizingMaskIntoConstraints = false;
        statusMark.WidthAnchor.ConstraintEqualTo(40).Active = true;
        statusMark.HeightAnchor.ConstraintEqualTo(40).Active = true;
        statusMark.Layer.CornerRadius = 20;
        statusMark.ClipsToBounds = true;
        var chevron = Theme.Symbol("chevron.right", 14, Theme.Muted);
        var statusRow = Theme.Row(12, statusMark, Theme.Column(2, statusTitle, statusDetail), chevron);
        statusRow.UserInteractionEnabled = false;
        statusRow.TranslatesAutoresizingMaskIntoConstraints = false;
        statusBanner.AddSubview(statusRow);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            statusRow.TopAnchor.ConstraintEqualTo(statusBanner.TopAnchor, 14), statusRow.BottomAnchor.ConstraintEqualTo(statusBanner.BottomAnchor, -14),
            statusRow.LeadingAnchor.ConstraintEqualTo(statusBanner.LeadingAnchor, 14), statusRow.TrailingAnchor.ConstraintEqualTo(statusBanner.TrailingAnchor, -14),
        });
        statusBanner.Layer.CornerRadius = 22;
        statusBanner.Layer.BorderWidth = 1;
        statusBanner.AccessibilityIdentifier = "settings-status";
        statusBanner.TouchUpInside += (_, __) => (statusFix ?? (() => ShowTab(2)))();

        UIView NavRow(string symbol, int colour, string name, out UILabel detail, Action tapped, string id, int page)
        {
            var b = new UIButton(UIButtonType.System) { AccessibilityIdentifier = id };
            navRows[page] = b;
            var tile = new UIView { BackgroundColor = Theme.Hex(colour) };
            tile.Layer.CornerRadius = 9;
            tile.TranslatesAutoresizingMaskIntoConstraints = false;
            tile.WidthAnchor.ConstraintEqualTo(32).Active = true;
            tile.HeightAnchor.ConstraintEqualTo(32).Active = true;
            var glyph = Theme.Symbol(symbol, 17, UIColor.White);
            tile.AddSubview(glyph);
            glyph.CenterXAnchor.ConstraintEqualTo(tile.CenterXAnchor).Active = true;
            glyph.CenterYAnchor.ConstraintEqualTo(tile.CenterYAnchor).Active = true;
            var label = Theme.Label(name, Theme.Font(16, UIFontWeight.Medium), Theme.Text);
            label.SetContentCompressionResistancePriority(1000, UILayoutConstraintAxis.Horizontal);
            detail = Theme.Label("", Theme.Font(13, UIFontWeight.Regular), Theme.Muted);
            detail.SetContentCompressionResistancePriority(250, UILayoutConstraintAxis.Horizontal);
            detail.LineBreakMode = UILineBreakMode.TailTruncation;
            detail.TextAlignment = UITextAlignment.Right;
            var row = Theme.Row(12, tile, label, new UIView(), detail, Theme.Symbol("chevron.right", 13, Theme.Dim));
            row.UserInteractionEnabled = false;
            row.TranslatesAutoresizingMaskIntoConstraints = false;
            b.AddSubview(row);
            NSLayoutConstraint.ActivateConstraints(new[]
            {
                row.TopAnchor.ConstraintEqualTo(b.TopAnchor, 11), row.BottomAnchor.ConstraintEqualTo(b.BottomAnchor, -11),
                row.LeadingAnchor.ConstraintEqualTo(b.LeadingAnchor, 14), row.TrailingAnchor.ConstraintEqualTo(b.TrailingAnchor, -14),
            });
            b.AccessibilityLabel = name;
            b.TouchUpInside += (_, __) => tapped();
            return b;
        }
        UIView Divider() { var d = new UIView { BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.08f) }; d.HeightAnchor.ConstraintEqualTo(1).Active = true; return d; }
        var nav = Theme.Column(0,
            NavRow("icloud.fill", 0x3D8BFF, "Cloud", out cloudDetail, () => ShowTab(1), "settings-cloud", 1), Divider(),
            NavRow("key.fill", 0xC99A12, "Keys and certificate", out keysDetail, () => ShowTab(2), "settings-keys", 2), Divider(),
            NavRow("shippingbox.fill", 0x34C77B, "Exported apps", out appsDetail, () => ShowTab(5), "settings-apps", 5), Divider(),
            NavRow("list.number", 0x8B6BFF, "Build queue", out queueDetail, () => ShowTab(6), "settings-queue", 6), Divider(),
            NavRow("text.alignleft", 0xE0508F, "Logs", out logsBadge, () => ShowTab(3), "settings-logs", 3));
        var navCard = Theme.Wrap(nav, 22, 0);
        navCard.ClipsToBounds = true;   // the open page's row is lit beside Settings on an iPad

        UILabel Section(string text) { var l = Theme.Label(text.ToUpperInvariant(), Theme.Font(12, UIFontWeight.Semibold), Theme.Muted); return l; }
        var build = BuildSection();
        var buildCard = Theme.Wrap(build, 22, 14);
        var storageCard = Theme.Wrap(settingsStorage, 22, 14);
        var about = Theme.Label("iBuilder · build " + GameAppBuild.Id, Theme.Font(12, UIFontWeight.Regular), Theme.Dim);
        about.TextAlignment = UITextAlignment.Center;

        var column = Theme.Column(14, title, statusBanner, navCard, Section("Build"), buildCard, Section("This " + GameAppUi.DeviceName), storageCard, about);
        column.SetCustomSpacing(6, column.ArrangedSubviews[3]);
        column.SetCustomSpacing(6, column.ArrangedSubviews[5]);
        var scroll = new UIScrollView { AlwaysBounceVertical = true };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        Theme.PinColumn(scroll, column, 4, 20);
        Fill(pages[4], scroll, 18);
    }

    /// The status banner: ready, or the first thing to fix (what the Keys page's checklist says, in one line).
    private void RefreshSettingsPage(UiFacts facts)
    {
        if (cloudDetail == null) return;
        var rows = GameAppUi.Checklist(facts).Where(r => r.Label == "prod.keys" || r.Label == "Signing certificate" || r.Label == "RAM unlocked"
                                                       || r.Label == "Extended addressing" || r.Label == "Play inside iBuilder").ToList();
        var problem = rows.FirstOrDefault(r => r.Level == CheckLevel.Missing) ?? rows.FirstOrDefault(r => r.Level == CheckLevel.Warn);
        UIColor tone;
        if (problem == null)
        {
            tone = Theme.Ok;
            statusTitle.Text = "Ready to build and play";
            statusDetail.Text = "Keys · certificate · RAM unlocked · extended memory";
            statusMark.Image = UIImage.GetSystemImage("checkmark", UIImageSymbolConfiguration.Create(UIFont.SystemFontOfSize(18, UIFontWeight.Heavy)));
            statusFix = () => ShowTab(2);
        }
        else
        {
            tone = problem.Level == CheckLevel.Missing ? Theme.Error : Theme.Warn;
            int more = rows.Count(r => r != problem && (r.Level == CheckLevel.Missing || r.Level == CheckLevel.Warn));
            statusTitle.Text = problem.Label + (more > 0 ? $" (+{more} more)" : "");
            statusDetail.Text = problem.Detail;
            statusMark.Image = UIImage.GetSystemImage(problem.Level == CheckLevel.Missing ? "xmark" : "exclamationmark", UIImageSymbolConfiguration.Create(UIFont.SystemFontOfSize(18, UIFontWeight.Heavy)));
            statusFix = problem.Label == "prod.keys" ? Import : problem.Label == "Signing certificate" ? ImportCertificate : () => ShowTab(2);
        }
        statusMark.BackgroundColor = tone;
        statusMark.TintColor = Theme.Ground;
        statusDetail.TextColor = tone.ColorWithAlpha(0.9f);
        statusBanner.BackgroundColor = tone.ColorWithAlpha(0.12f);
        statusBanner.Layer.BorderColor = tone.ColorWithAlpha(0.35f).CGColor;
        statusBanner.AccessibilityLabel = statusTitle.Text + ". " + statusDetail.Text;

        var sources = cloudSources;
        cloudDetail.Text = sources.Count == 0 ? "Not set up" : string.Join(" · ", sources.Select(x => x.Kind == CloudKind.Nas ? "NAS" : Cloud.ProviderName(x.Location)).Distinct());
        var certificate = rows.FirstOrDefault(r => r.Label == "Signing certificate");
        keysDetail.Text = certificate == null ? "" : certificate.Level == CheckLevel.Ok ? "ready" : "no certificate";
        appsDetail.Text = AppsSummary();
        logsBadge.Text = logErrors > 0 ? $"{logErrors} error{(logErrors == 1 ? "" : "s")}" : "";
        logsBadge.TextColor = logErrors > 0 ? Theme.Error : Theme.Muted;
        settingsStorage.Set(CurrentStorageSegments());
    }

    // ---- Logs: a timeline ------------------------------------------------------------------------------------

    private const int MaxLogLines = 6000;
    private readonly List<(string Time, string Text, GameAppUi.LogKind Kind)> logLines = new List<(string, string, GameAppUi.LogKind)>();
    private string logPartial = "";
    private bool onlyProblems;
    private int logErrors, logWarnings;
    private readonly UILabel tileTime = new UILabel(), tileErrors = new UILabel(), tileWarnings = new UILabel();
    private UISwitch problemsSwitch;

    private readonly UITableView logTable = new UITableView(CGRect.Empty, UITableViewStyle.Plain);
    private readonly LogSource logSource = new LogSource();

    private void PrepareLog()
    {
        logTable.BackgroundColor = UIColor.Clear;
        logTable.SeparatorStyle = UITableViewCellSeparatorStyle.None;
        logTable.RowHeight = UITableView.AutomaticDimension;
        logTable.EstimatedRowHeight = 44;
        logTable.IndicatorStyle = UIScrollViewIndicatorStyle.White;
        logTable.AllowsSelection = false;
        logTable.ContentInset = new UIEdgeInsets(6, 0, 24, 0);
        logTable.AccessibilityIdentifier = "log-timeline";
        logTable.RegisterClassForCellReuse(typeof(TimelineCell), TimelineCell.Id);
        logTable.Source = logSource;
    }

    private void BuildLogsPage()
    {
        var c = UIButtonConfiguration.PlainButtonConfiguration;
        c.Image = UIImage.GetSystemImage("square.and.arrow.up");
        c.BaseForegroundColor = Theme.Text;
        export.Configuration = c;
        export.SetTitle(null, UIControlState.Normal);
        export.AccessibilityLabel = "Share the logs";
        export.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.10f);
        export.Layer.CornerRadius = 22;
        export.Layer.BorderWidth = 1;
        export.Layer.BorderColor = Theme.GlassLine.CGColor;
        export.WidthAnchor.ConstraintEqualTo(44).Active = true;
        export.HeightAnchor.ConstraintEqualTo(44).Active = true;
        var jump = Theme.Round("arrow.down.to.line", "Jump to the first error");
        jump.TouchUpInside += (_, __) => JumpToFirstError();
        var header = SubHeader("Build log", jump, export);

        UIView Tile(UILabel value, string caption, UIColor color, UIColor fill, UIColor edge)
        {
            value.Font = Theme.Rounded(22, UIFontWeight.Bold);
            value.TextColor = color;
            var cap = Theme.Label(caption, Theme.Font(12, UIFontWeight.Medium), color == Theme.Text ? Theme.Muted : color.ColorWithAlpha(0.8f));
            var card = Theme.Wrap(Theme.Column(0, value, cap), 18, 12, fill);
            if (edge != null) card.Layer.BorderColor = edge.CGColor;
            return card;
        }
        var tiles = new UIStackView(new[]
        {
            Tile(tileTime, "last run", Theme.Text, null, null),
            Tile(tileErrors, "errors", Theme.Error, Theme.Hex(0xFF6B6B, 0.16f), Theme.Hex(0xFF6B6B, 0.35f)),
            Tile(tileWarnings, "warnings", Theme.Warn, Theme.Hex(0xFFC870, 0.14f), Theme.Hex(0xFFC870, 0.32f)),
        }) { Axis = UILayoutConstraintAxis.Horizontal, Distribution = UIStackViewDistribution.FillEqually, Spacing = 8 };

        problemsSwitch = new UISwitch { OnTintColor = Theme.Ok, AccessibilityIdentifier = "logs-problems-only" };
        problemsSwitch.ValueChanged += (_, __) => { onlyProblems = problemsSwitch.On; RebuildLog(); };
        var switchLabel = Theme.Label("Errors and warnings only", Theme.Font(15, UIFontWeight.Semibold), Theme.Text);
        var switchRow = Theme.Wrap(Theme.Row(8, switchLabel, new UIView(), problemsSwitch), 18, 12, UIColor.FromWhiteAlpha(1f, 0.07f));

        var column = Theme.Column(12, header, tiles, switchRow, logTable);
        foreach (var v in new UIView[] { header, tiles, switchRow }) { v.SetContentHuggingPriority(1000, UILayoutConstraintAxis.Vertical); v.SetContentCompressionResistancePriority(1000, UILayoutConstraintAxis.Vertical); }
        logTable.SetContentHuggingPriority(1, UILayoutConstraintAxis.Vertical);
        logTable.SetContentCompressionResistancePriority(1, UILayoutConstraintAxis.Vertical);
        RefreshLogTiles();
        Fill(pages[3], column, 16);
    }

    private void RefreshLogTiles()
    {
        tileTime.Text = startedAt == default ? "—" : GameAppUi.Clock((running ? DateTime.Now : lastRunEnd == default ? DateTime.Now : lastRunEnd) - startedAt);
        tileErrors.Text = logErrors.ToString();
        tileWarnings.Text = logWarnings.ToString();
    }

    private DateTime lastRunEnd;

    private void Append(string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        // The same lines, on disk (gameapp.ui.log), bounded: a device run is diagnosed from the files it left.
        try
        {
            if (uiLog == null)
                uiLog = new StreamWriter(Path.Combine(GameAppMain.Documents, "gameapp.ui.log"), append: true) { AutoFlush = true };
            uiLog.Write(text);
            if (uiLog.BaseStream.Length > uiLogNextTrim) RotateUiLog();
        }
        catch { }
        logPartial += text;
        int nl, added = 0;
        var time = DateTime.Now.ToString("HH:mm");
        while ((nl = logPartial.IndexOf('\n')) >= 0)
        {
            var line = logPartial.Substring(0, nl).TrimEnd('\r');
            logPartial = logPartial.Substring(nl + 1);
            if (line.Trim().Length == 0) continue;
            var kind = GameAppUi.KindOfLine(line);
            logLines.Add((time, line, kind));
            added++;
            if (kind == GameAppUi.LogKind.Error) logErrors++;
            else if (kind == GameAppUi.LogKind.Warning) logWarnings++;
        }
        if (logLines.Count > MaxLogLines) logLines.RemoveRange(0, logLines.Count - MaxLogLines * 3 / 4);
        if (added > 0 && !pages[3].Hidden) RebuildLog(keepPlace: true);
        else if (added > 0) logDirty = true;
        RefreshLogTiles();
    }

    private bool logDirty = true;

    /// The rows again from the lines (the filter, the headers). While the page is hidden nothing is redrawn: it is
    /// drawn when it shows (ShowTab), so a long build does not lay out a table nobody sees.
    private void RebuildLog(bool keepPlace = false)
    {
        bool atEnd = !keepPlace || logTable.ContentOffset.Y + logTable.Bounds.Height >= logTable.ContentSize.Height - 120;
        logSource.Rows = GameAppUi.Timeline(logLines, onlyProblems);
        logTable.ReloadData();
        logDirty = false;
        if (atEnd) ScrollLogToEnd();
    }

    private void ScrollLogToEnd()
    {
        if (logDirty) { RebuildLog(); return; }
        logTable.LayoutIfNeeded();
        var n = logSource.Rows.Count;
        if (n > 0) logTable.ScrollToRow(NSIndexPath.FromRowSection(n - 1, 0), UITableViewScrollPosition.Bottom, false);
    }

    private void JumpToFirstError()
    {
        if (logDirty) RebuildLog();
        var i = logSource.Rows.FindIndex(r => r.Kind == GameAppUi.LogKind.Error);
        if (i < 0)
        {
            var a = UIAlertController.Create("No errors", "Nothing went wrong in what this log holds.", UIAlertControllerStyle.Alert);
            a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
            PresentWhenFree(a);
            return;
        }
        logTable.ScrollToRow(NSIndexPath.FromRowSection(i, 0), UITableViewScrollPosition.Middle, true);
    }

    private sealed class LogSource : UITableViewSource
    {
        public List<GameAppUi.TimelineRow> Rows = new List<GameAppUi.TimelineRow>();
        public override nint RowsInSection(UITableView tableView, nint section) => Rows.Count;
        public override UITableViewCell GetCell(UITableView tableView, NSIndexPath indexPath)
        {
            var cell = (TimelineCell) tableView.DequeueReusableCell(TimelineCell.Id, indexPath);
            int i = indexPath.Row;
            cell.Set(Rows[i], i == Rows.Count - 1);
            return cell;
        }
    }

    // ---- the build screen -----------------------------------------------------------------------------------

    private readonly Dictionary<int, DateTime> stepStarts = new Dictionary<int, DateTime>();
    private bool buildFinished;
    private GameBuildViewController buildScreen;

    private void NoteStep(string stageLine)
    {
        var s = GameAppUi.ParseStage(stageLine);
        if (s.Valid && s.Of == 5)
            for (int i = 1; i <= s.Step; i++)
                if (!stepStarts.ContainsKey(i)) stepStarts[i] = DateTime.Now;
    }

    private void ShowBuildScreen()
    {
        if (buildScreen != null && buildScreen.PresentingViewController != null) return;
        buildScreen = new GameBuildViewController
        {
            // The log opens over the library, and its back button brings the build screen back (logsBack).
            OnLog = OpenLogFromBuild,
            OnPrimary = () =>
            {
                if (failedRun && !running) { OpenLogFromBuild(toError: true); return; }   // "See what stopped it"
                buildScreen?.DismissViewController(true, () => { if (!running) PrimaryTapped(); });
            },
            OnNotify = ToggleNotify,
        };
        buildScreen.ModalPresentationStyle = UIModalPresentationStyle.FullScreen;
        RefreshBuildScreen(force: true);   // not presented yet: the first model goes in now
        PresentWhenFree(buildScreen);
    }

    /// The build screen's live log: the log page, whose back button brings the build screen back.
    private void OpenLogFromBuild() => OpenLogFromBuild(false);

    private void OpenLogFromBuild(bool toError)
    {
        // The log page is put under the build screen first, then the screen slides away onto it: nothing waits on the
        // dismissal's completion (a completion that never came left the library showing).
        logsBack = ShowBuildScreen;
        ShowTab(3);
        void Place()
        {
            // Rows size themselves: placed before the page is laid out, the scroll lands mid-log.
            logTable.LayoutIfNeeded();
            int last = toError ? logSource.Rows.FindLastIndex(r => r.Kind == GameAppUi.LogKind.Error) : -1;
            if (last >= 0) logTable.ScrollToRow(NSIndexPath.FromRowSection(last, 0), UITableViewScrollPosition.Middle, false);
            else ScrollLogToEnd();
        }
        if (buildScreen?.PresentingViewController != null) buildScreen.DismissViewController(true, Place); else Place();
    }

    /// The lines the build screen's live log can show: from the log's tail, scanning backwards. A failed run keeps its
    /// last error and the lines around it; otherwise the last non-stage lines (a counter's lines fold into one row).
    private (int From, int To) LiveLogWindow()
    {
        int n = logLines.Count;
        if (failedRun)
        {
            int e = logLines.FindLastIndex(l => l.Kind == GameAppUi.LogKind.Error);
            if (e >= 0) return (Math.Max(0, e - 30), Math.Min(n, e + 30));
        }
        int from = n, seen = 0;
        while (from > 0 && seen < 200) { from--; if (logLines[from].Kind != GameAppUi.LogKind.Stage) seen++; }
        return (from, n);
    }

    private void RefreshBuildScreen(bool force = false)
    {
        RefreshLogTiles();
        // Nobody is looking at a screen that is not presented: the ticker's second-by-second refresh stops there.
        if (buildScreen == null || (!force && buildScreen.PresentingViewController == null)) return;
        var facts = GatherFacts();
        var game = selected;
        var entry = entries.FirstOrDefault(e => e.Name == game);
        var now = running ? DateTime.Now : lastRunEnd == default ? DateTime.Now : lastRunEnd;
        var steps = GameAppUi.BuildSteps(lastStage, stepStarts, now, finished: buildFinished && !failedRun);
        int at = steps.FindIndex(r => r.State == GameAppUi.StepState.Now);
        if (at < 0) at = steps.Count - 1;
        var names = Dumps().Select(Path.GetFileName).ToList();
        string meta = game == null ? "" : GameAppUi.ContentLine(game, names) ?? GameAppUi.SizeText(SafeLength(Path.Combine(GameAppMain.Documents, game)));
        var fraction = buildFinished && !failedRun ? 1.0 : GameAppUi.ParseStage(lastStage).Fraction;
        var primary = game != null ? GameAppUi.GameAction(game, facts) : null;
        var window = LiveLogWindow();
        var rows = GameAppUi.Timeline(logLines.Skip(window.From).Take(window.To - window.From), false).Where(r => r.Kind != GameAppUi.LogKind.Stage).ToList();
        int lastError = failedRun ? rows.FindLastIndex(r => r.Kind == GameAppUi.LogKind.Error) : -1;
        var live = (lastError >= 0 ? rows.Skip(Math.Max(0, lastError - 1)).Take(3) : rows.TakeLast(3)).Select(r => (r.Text, r.Kind)).ToList();
        long left = GameAppMain.AvailableMiBValue();
        buildScreen.Update(new GameBuildViewController.Model(
            game != null ? GameAppUi.DisplayName(game) : "Build",
            entry?.Icon ?? (game != null ? CoverOf(game, facts) : null),
            meta, steps, at, fraction, running, failedRun,
            running ? GameAppUi.LeftText(GameAppUi.TimeLeft(lastStage, stepStarts, DateTime.Now, DateTime.Now - startedAt)) : failedRun ? "stopped" : "done",
            (running ? GameAppUi.FormatElapsed(DateTime.Now - startedAt) + " so far" : startedAt == default ? "" : "took " + GameAppUi.FormatElapsed(lastRunEnd - startedAt)),
            facts.IncreasedMemory == 1 || facts.AvailableMiB > 4500, facts.ExtendedAddressing == 1, ((left > 0 ? $"{left:N0} MiB free" : "") + HeatSuffix()).TrimStart(' ', '·'),
            live, notifyWhenDone,
            running ? "Play when it is done" : failedRun ? "See what stopped it" : primary?.Title ?? "Close",
            !running,
            currentTint ?? Theme.DefaultTint));
    }

    // ---- "Notify me": a notification when the build ends (the app may be in the background meanwhile) ------------

    private bool notifyWhenDone;

    private void ToggleNotify()
    {
        if (notifyWhenDone) { notifyWhenDone = false; RefreshBuildScreen(); return; }
        UserNotifications.UNUserNotificationCenter.Current.RequestAuthorization(
            UserNotifications.UNAuthorizationOptions.Alert | UserNotifications.UNAuthorizationOptions.Sound, (granted, error) =>
                BeginInvokeOnMainThread(() =>
                {
                    notifyWhenDone = granted;
                    if (!granted) Write("notifications are off for iBuilder (Settings › Notifications): no notification when the build ends");
                    RefreshBuildScreen();
                }));
    }

    private void NotifyBuildEnded()
    {
        if (!notifyWhenDone) return;
        notifyWhenDone = false;
        var content = new UserNotifications.UNMutableNotificationContent
        {
            Title = selected != null ? GameAppUi.DisplayName(selected) : "iBuilder",
            Body = failedRun ? "The build stopped: open iBuilder to see why." : "Built: ready to play.",
            Sound = UserNotifications.UNNotificationSound.Default,
        };
        var request = UserNotifications.UNNotificationRequest.FromIdentifier("build-ended", content, null);
        UserNotifications.UNUserNotificationCenter.Current.AddNotificationRequest(request, null);
    }

    private bool failedRun;
    private string lastRunGame;   // the game the last build was for: its card stays under it once the build is over

    private static long SafeLength(string path)
    {
        try { return File.Exists(path) ? new FileInfo(path).Length : 0; } catch (Exception) { return 0; }
    }

    // ---- Cloud ----------------------------------------------------------------------------------------------

    private const string NasId = "nas";
    private List<CloudSource> folderSources = new List<CloudSource>();
    private List<CloudFile> cloudFiles = new List<CloudFile>();
    private readonly Dictionary<string, double> downloads = new Dictionary<string, double>();
    private readonly Dictionary<string, string> sourceState = new Dictionary<string, string>();
    private readonly UIStackView cloudRows = new UIStackView();
    private Timer downloadTicker;
    private int refreshing;

    private List<CloudSource> cloudSources
    {
        get
        {
            var list = new List<CloudSource>();
            var nas = LoadNas();
            if (nas.IsComplete) list.Add(new CloudSource(NasId, CloudKind.Nas, "NAS", nas.Folder));
            list.AddRange(folderSources);
            return list;
        }
    }

    private static string AppSupport
    {
        get
        {
            var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Personal), "..", "Library", "Application Support");
            Directory.CreateDirectory(dir);
            return dir;
        }
    }

    private static string CloudStatePath => Path.Combine(AppSupport, "gameapp.cloud.tsv");
    private static string BookmarkPath(string id) => Path.Combine(AppSupport, "gameapp.cloud." + id + ".bookmark");

    private void LoadCloud()
    {
        try
        {
            if (!File.Exists(CloudStatePath)) return;
            var (sources, files) = Cloud.Parse(File.ReadAllText(CloudStatePath));
            folderSources = sources.Where(s => s.Kind == CloudKind.Folder).ToList();
            var ids = new HashSet<string>(folderSources.Select(s => s.Id)) { NasId };
            cloudFiles = files.Where(f => ids.Contains(f.SourceId)).ToList();
        }
        catch (Exception error) { Write($"cloud: the saved list could not be read ({error.Message})"); }
    }

    private void SaveCloud()
    {
        try
        {
            var nas = LoadNas();
            var keep = cloudFiles.Where(f => f.SourceId != NasId || nas.IsComplete).ToList();
            File.WriteAllText(CloudStatePath, Cloud.Serialize(new[] { new CloudSource(NasId, CloudKind.Nas, "NAS", nas.Folder) }.Concat(folderSources), keep));
        }
        catch (Exception error) { Write($"cloud: the list could not be saved ({error.Message})"); }
    }

    private void BuildCloudPage()
    {
        var refresh = Theme.Round("arrow.triangle.2.circlepath", "Refresh every source");
        refresh.TouchUpInside += (_, __) => RefreshCloud(null);
        var header = SubHeader("Cloud", refresh);
        var note = Theme.Label("Games (.nsp, .xci, .nsz, .nup) and exported apps (.ipa) in these folders show in the library, greyed until you "
                               + "download them. Google Drive, OneDrive and Dropbox appear here once their app is installed and turned on in the Files app.",
                               Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0);
        cloudRows.Axis = UILayoutConstraintAxis.Vertical;
        cloudRows.Spacing = 10;
        var add = Theme.Pill("Add a folder (iCloud Drive, Google Drive, OneDrive…)", "folder.badge.plus", true, 52);
        add.AccessibilityIdentifier = "cloud-add-folder";
        add.TouchUpInside += (_, __) => AddFolderSource();
        var column = Theme.Column(14, header, note, cloudRows, add);
        var scroll = new UIScrollView { AlwaysBounceVertical = true };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        Theme.PinColumn(scroll, column, 4, 20);
        Fill(pages[1], scroll, 18);
        RefreshCloudPage();
    }

    private void RefreshCloudPage()
    {
        foreach (var v in cloudRows.ArrangedSubviews.ToArray()) { cloudRows.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }

        // The NAS: its server, its folder, what it holds.
        var nas = LoadNas();
        var nasTitle = Theme.Row(10, Theme.Symbol("externaldrive.connected.to.line.below.fill", 24),
                                 Theme.Label("NAS (SFTP)", Theme.Font(17, UIFontWeight.Semibold), Theme.Text), new UIView());
        nasLabel = Theme.Label("", Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0);
        RefreshNasLabel();
        var nasCount = cloudFiles.Count(f => f.SourceId == NasId);
        var nasState = Theme.Label(nas.IsComplete ? (sourceState.TryGetValue(NasId, out var st) ? st : $"{nasCount} file(s) in {nas.Folder}") : "Not set up yet",
                                   Theme.Font(13, UIFontWeight.Medium), Theme.Text, 0);
        UIButton Small(string text, Action tapped)
        {
            var b = Theme.Pill(text, null, false, 40);
            b.TouchUpInside += (_, __) => tapped();
            return b;
        }
        var nasButtons = new UIStackView(new UIView[] { Small("Set up…", ConfigureNas), Small("Folder…", ChooseNasFolder), Small("Test", TestNas) })
        { Axis = UILayoutConstraintAxis.Horizontal, Spacing = 8, Distribution = UIStackViewDistribution.FillEqually };
        cloudRows.AddArrangedSubview(Theme.Wrap(Theme.Column(8, nasTitle, nasLabel, nasState, nasButtons), 22, 14));

        // The Files folders.
        foreach (var source in folderSources)
        {
            var s = source;
            var icon = Cloud.ProviderName(s.Location) switch
            {
                "iCloud Drive" => "icloud.fill",
                "Google Drive" => "externaldrive.fill.badge.icloud",
                "OneDrive" => "cloud.fill",
                "Dropbox" => "shippingbox.fill",
                _ => "folder.fill",
            };
            var count = cloudFiles.Count(f => f.SourceId == s.Id);
            var head = Theme.Row(10, Theme.Symbol(icon, 24), Theme.Label(s.Name, Theme.Font(17, UIFontWeight.Semibold), Theme.Text, 2), new UIView());
            var line = Theme.Label(sourceState.TryGetValue(s.Id, out var state) ? state : $"{count} file(s)", Theme.Font(13, UIFontWeight.Medium), Theme.Muted, 0);
            var refresh = Small("Refresh", () => RefreshCloud(s.Id));
            var remove = Small("Remove", () =>
            {
                var a = UIAlertController.Create("Remove this folder?", $"{s.Name} stops showing in the library. Nothing is deleted, there or here.", UIAlertControllerStyle.Alert);
                a.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
                a.AddAction(UIAlertAction.Create("Remove", UIAlertActionStyle.Destructive, _ =>
                {
                    folderSources = folderSources.Where(x => x.Id != s.Id).ToList();
                    cloudFiles = cloudFiles.Where(f => f.SourceId != s.Id).ToList();
                    try { File.Delete(BookmarkPath(s.Id)); } catch (Exception) { }
                    SaveCloud();
                    RefreshCloudPage();
                    RefreshScreen();
                }));
                PresentWhenFree(a);
            });
            var buttons = new UIStackView(new UIView[] { refresh, remove }) { Axis = UILayoutConstraintAxis.Horizontal, Spacing = 8, Distribution = UIStackViewDistribution.FillEqually };
            cloudRows.AddArrangedSubview(Theme.Wrap(Theme.Column(8, head, line, buttons), 22, 14));
        }
    }

    /// Where a local file can be sent: the NAS (when set up) and every folder.
    private List<CloudSource> UploadTargets() => GameExport.IsPlayer ? new List<CloudSource>() : cloudSources;

    private void AddFolderSource()
    {
        var picker = new UIDocumentPickerViewController(new[] { "public.folder" }, UIDocumentPickerMode.Open);
        picker.Delegate = new FolderPickerDelegate(FolderPicked);
        PresentWhenFree(picker);
    }

    private void FolderPicked(NSUrl url)
    {
        if (url == null) return;
        bool scoped = url.StartAccessingSecurityScopedResource();
        try
        {
            var data = url.CreateBookmarkData(NSUrlBookmarkCreationOptions.MinimalBookmark, null, null, out NSError error);
            if (data == null) { Write($"cloud: that folder cannot be kept ({error?.LocalizedDescription})"); return; }
            var id = Cloud.NewId(CloudKind.Folder, cloudSources);
            data.Save(BookmarkPath(id), true);
            var source = new CloudSource(id, CloudKind.Folder, Cloud.SourceName(url.Path), url.Path);
            folderSources.Add(source);
            SaveCloud();
            Write($"cloud: added {source.Name} ({url.Path})");
            RefreshCloudPage();
            RefreshCloud(id);
        }
        finally { if (scoped) url.StopAccessingSecurityScopedResource(); }
    }

    private sealed class FolderPickerDelegate : NSObject, IUIDocumentPickerDelegate
    {
        private readonly Action<NSUrl> picked;
        public FolderPickerDelegate(Action<NSUrl> picked) => this.picked = picked;
        [Export("documentPicker:didPickDocumentsAtURLs:")]
        public void DidPickDocumentAtUrls(UIDocumentPickerViewController controller, NSUrl[] urls) => picked(urls?.FirstOrDefault());
    }

    // ---- a game's update and DLC: what it has, add files (several at once), or find them in a folder ------------

    private UIButton contentAdd;
    private UILabel contentTitle;
    private UIView contentCardView;
    private readonly UIStackView contentList = new UIStackView();

    /// The card under the game: its update and each DLC by name (a dot each; added since the build: marked), and
    /// Add's menu.
    private void FillContentCard(string game, UiFacts facts)
    {
        foreach (var v in contentList.ArrangedSubviews.ToArray()) { contentList.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        var names = Dumps().Select(Path.GetFileName).ToList();
        var update = GameAppUi.UpdateFor(game, names);
        var dlcs = GameAppUi.DlcFor(game, names);
        UIView Line(string symbol, string text, UIColor tint)
        {
            var l = Theme.Label(text, Theme.Font(14, UIFontWeight.Regular), Theme.Text, 1);
            l.LineBreakMode = UILineBreakMode.TailTruncation;
            return Theme.Row(8, Theme.Symbol(symbol, 15, tint), l);
        }
        if (update == null && dlcs.Count == 0)
            contentList.AddArrangedSubview(Theme.Label("The game as dumped. Add its update or DLC to build them in.", Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0));
        if (update != null) { var u = GameAppUi.UpdateLabel(update); contentList.AddArrangedSubview(Line("arrow.up.circle.fill", char.ToUpperInvariant(u[0]) + u.Substring(1), Theme.Ok)); }
        foreach (var d in dlcs.Take(6)) contentList.AddArrangedSubview(Line("puzzlepiece.extension.fill", GameAppUi.DlcLabel(d), Theme.Hex(0x9DB7FF)));
        if (dlcs.Count > 6) contentList.AddArrangedSubview(Theme.Label($"and {dlcs.Count - 6} more DLC", Theme.Font(13, UIFontWeight.Regular), Theme.Muted));
        var inCloud = GameAppUi.RelatedContent(game, cloudFiles.Select(f => f.Name)).Where(n => !names.Contains(n, StringComparer.OrdinalIgnoreCase)).ToList();
        if (inCloud.Count > 0) contentList.AddArrangedSubview(Line("icloud", $"{inCloud.Count} more in the cloud", Theme.Muted));
        if (GameAppUi.ContentChangedSinceBuild(game, names, facts))
            contentList.AddArrangedSubview(Theme.Label("Added since the last build: rebuild to include it.", Theme.Font(12, UIFontWeight.Medium), Theme.Warn, 0));
        contentTitle.Text = update == null && dlcs.Count == 0 ? "Update & DLC" : $"Update & DLC · {(update != null ? 1 : 0) + dlcs.Count}";
        var items = new List<UIMenuElement>
        {
            UIAction.Create("Add files… (pick several)", UIImage.GetSystemImage("doc.badge.plus"), null, _ => Import()),
            UIAction.Create("Find them in a folder…", UIImage.GetSystemImage("folder.badge.questionmark"), null, _ =>
            {
                var picker = new UIDocumentPickerViewController(new[] { "public.folder" }, UIDocumentPickerMode.Open);
                picker.Delegate = new FolderPickerDelegate(url => ScanFolderForContent(game, url));
                PresentWhenFree(picker);
            }),
        };
        if (inCloud.Count > 0)
            items.Add(UIAction.Create($"Download {inCloud.Count} from the cloud", UIImage.GetSystemImage("icloud.and.arrow.down"), null, _ =>
                Download(inCloud.Select(n => Cloud.CloudCopyOf(n, cloudFiles)).Where(f => f != null).ToList(), GameAppMain.Documents)));
        contentAdd.Menu = UIMenu.Create(items.ToArray());
        contentAdd.Enabled = !facts.Running;
    }

    /// Every .nsp/.nsz/.xci in the folder and its subfolders whose title id says it is this game's update or DLC
    /// (GameAppUi.RelatedContent); after a yes, copied into Documents (a coordinated read: an iCloud or Drive file
    /// is downloaded first). The folder itself is left as it is.
    private void ScanFolderForContent(string game, NSUrl folder)
    {
        if (folder == null) return;
        Write($"\n=== looking for {GameAppUi.DisplayName(game)}'s update and DLC in {folder.Path}");
        System.Threading.Tasks.Task.Run(() =>
        {
            List<string> found = null; string error = null;
            bool scoped = folder.StartAccessingSecurityScopedResource();
            try
            {
                var all = Directory.EnumerateFiles(folder.Path, "*", SearchOption.AllDirectories).Take(20000)
                                   .Select(p => (Path: p, Name: Cloud.RealName(Path.GetFileName(p)))).Where(x => Cloud.IsGameFile(x.Name)).ToList();
                var match = new HashSet<string>(GameAppUi.RelatedContent(game, all.Select(x => x.Name)), StringComparer.OrdinalIgnoreCase);
                var have = new HashSet<string>(Dumps().Select(Path.GetFileName), StringComparer.OrdinalIgnoreCase);
                found = all.Where(x => match.Contains(x.Name) && !have.Contains(x.Name))
                           .Select(x => Path.Combine(Path.GetDirectoryName(x.Path), x.Name)).Distinct().ToList();
            }
            catch (Exception e) { error = e.Message; }
            finally { if (scoped) folder.StopAccessingSecurityScopedResource(); }
            BeginInvokeOnMainThread(() =>
            {
                if (error != null || found.Count == 0)
                {
                    var none = UIAlertController.Create(error != null ? "Could not read the folder" : "Nothing new found",
                        error ?? $"No update or DLC of {GameAppUi.DisplayName(game)} that this {GameAppUi.DeviceName} does not have yet (matched by title id).", UIAlertControllerStyle.Alert);
                    none.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                    PresentWhenFree(none);
                    return;
                }
                var listing = string.Join("\n", found.Select(p => "• " + (GameAppUi.ContentKindOf(Path.GetFileName(p)) == GameAppUi.ContentKind.Update ? GameAppUi.UpdateLabel(Path.GetFileName(p)) : "DLC " + GameAppUi.DlcLabel(Path.GetFileName(p)))));
                var ask = UIAlertController.Create($"Found {found.Count} for {GameAppUi.DisplayName(game)}", listing, UIAlertControllerStyle.Alert);
                ask.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
                ask.AddAction(UIAlertAction.Create("Add them", UIAlertActionStyle.Default, _ => CopyContentIn(folder, found)));
                PresentWhenFree(ask);
            });
        });
    }

    private void CopyContentIn(NSUrl folder, List<string> paths)
    {
        var key = Path.GetFileName(paths[0]);
        downloads[key] = 0;
        StartDownloadTicker();
        nint task = UIApplication.SharedApplication.BeginBackgroundTask("content", null);
        System.Threading.Tasks.Task.Run(() =>
        {
            var errors = new List<string>();
            bool scoped = folder.StartAccessingSecurityScopedResource();
            try
            {
                for (int i = 0; i < paths.Count; i++)
                {
                    var p = paths[i];
                    var dest = Path.Combine(GameAppMain.Documents, Path.GetFileName(p));
                    try
                    {
                        Exception failure = null;
                        new NSFileCoordinator().CoordinateRead(NSUrl.FromFilename(p), (NSFileCoordinatorReadingOptions) 0, out NSError err, readable =>
                        {
                            try
                            {
                                int index = i;
                                CopyWithProgress(readable.Path, dest, (done, total) =>
                                    BeginInvokeOnMainThread(() => { if (downloads.ContainsKey(key)) downloads[key] = (index + done / (double) Math.Max(1, total)) / paths.Count; }));
                            }
                            catch (Exception e) { failure = e; }
                        });
                        if (err != null) throw new InvalidOperationException(err.LocalizedDescription);
                        if (failure != null) throw failure;
                    }
                    catch (Exception e) { errors.Add($"{Path.GetFileName(p)}: {e.Message}"); }
                }
            }
            finally { if (scoped) folder.StopAccessingSecurityScopedResource(); }
            BeginInvokeOnMainThread(() =>
            {
                downloads.Remove(key);
                UIApplication.SharedApplication.EndBackgroundTask(task);
                Write(errors.Count == 0 ? $"added {paths.Count} file(s): {string.Join(", ", paths.Select(Path.GetFileName))} — rebuild the game to include them"
                                        : "some were not added: " + string.Join("; ", errors));
                var done = UIAlertController.Create(errors.Count == 0 ? "Added" : "Not all were added",
                    errors.Count == 0 ? $"{paths.Count} file(s) added. Rebuild the game (its settings › Rebuild) to include them." : string.Join("\n", errors), UIAlertControllerStyle.Alert);
                done.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                PresentWhenFree(done);
                RefreshGames();
                RefreshScreen();
            });
        });
    }

    /// The folder a bookmark points at, or null (moved, or the provider is gone). The caller starts and stops access.
    private static NSUrl ResolveFolder(CloudSource s, out string why)
    {
        why = null;
        try
        {
            var data = NSData.FromFile(BookmarkPath(s.Id));
            if (data == null) { why = "its bookmark is missing: remove it and add the folder again"; return null; }
            var url = NSUrl.FromBookmarkData(data, NSUrlBookmarkResolutionOptions.WithoutUI, null, out bool stale, out NSError error);
            if (url == null) { why = error?.LocalizedDescription ?? "the folder cannot be reached"; return null; }
            if (stale)
            {
                bool scoped = url.StartAccessingSecurityScopedResource();
                try { url.CreateBookmarkData(NSUrlBookmarkCreationOptions.MinimalBookmark, null, null, out _)?.Save(BookmarkPath(s.Id), true); }
                finally { if (scoped) url.StopAccessingSecurityScopedResource(); }
            }
            return url;
        }
        catch (Exception e) { why = e.Message; return null; }
    }

    /// Lists every source (or one) off the main thread, then redraws.
    private void RefreshCloud(string only)
    {
        if (GameExport.IsPlayer) return;
        if (Interlocked.CompareExchange(ref refreshing, 1, 0) != 0) return;
        var sources = cloudSources.Where(s => only == null || s.Id == only).ToList();
        if (sources.Count == 0) { refreshing = 0; return; }
        var nas = LoadNas();
        var password = NasPassword();
        foreach (var s in sources) sourceState[s.Id] = "Refreshing…";
        RefreshCloudPage();
        System.Threading.Tasks.Task.Run(() =>
        {
            var results = new List<(string Id, List<CloudFile> Files, string Error)>();
            foreach (var s in sources)
            {
                try
                {
                    if (s.Kind == CloudKind.Nas)
                    {
                        if (string.IsNullOrEmpty(password)) { results.Add((s.Id, null, "no password saved: Set up…")); continue; }
                        results.Add((s.Id, Nas.List(nas, Nas.Password(nas.User, password), nas.Folder, NasId, null), null));
                    }
                    else
                    {
                        var url = ResolveFolder(s, out var why);
                        if (url == null) { results.Add((s.Id, null, why)); continue; }
                        bool scoped = url.StartAccessingSecurityScopedResource();
                        try
                        {
                            var files = Directory.EnumerateFiles(url.Path).Select(p =>
                            {
                                var name = Path.GetFileName(p);
                                long size = 0;
                                try { size = name.EndsWith(".icloud", StringComparison.OrdinalIgnoreCase) ? 0 : new FileInfo(p).Length; } catch (Exception) { }
                                return new CloudFile(s.Id, name, size, Cloud.RealName(name));
                            }).ToList();
                            results.Add((s.Id, files, null));
                        }
                        finally { if (scoped) url.StopAccessingSecurityScopedResource(); }
                    }
                }
                catch (Exception e) { results.Add((s.Id, null, e.Message)); }
            }
            BeginInvokeOnMainThread(() =>
            {
                refreshing = 0;
                foreach (var (id, files, error) in results)
                {
                    if (files != null)
                    {
                        cloudFiles = Cloud.Replace(cloudFiles, id, files);
                        sourceState[id] = $"{cloudFiles.Count(f => f.SourceId == id)} file(s) · refreshed {DateTime.Now:HH:mm}";
                    }
                    else
                    {
                        sourceState[id] = "Could not refresh: " + error;
                        Write($"cloud: {id} could not be listed: {error}");
                    }
                }
                SaveCloud();
                RefreshCloudPage();
                RefreshScreen();
                FetchCloudIcons();
            });
        });
    }

    private int cloudIconsRunning;

    /// The covers of the games only in the cloud: read from the dump where it lies (GameAppIcons.cs: the package's
    /// header and its control NCA, a few hundred KB over SFTP or from a Files folder; an iCloud placeholder is skipped
    /// rather than downloaded whole), kept as games/<id>/icon.jpg like a local game's. Needs prod.keys. A dump that
    /// gives none gets icon.none, tried again only with newer keys.
    private void FetchCloudIcons()
    {
        var keys = Path.Combine(GameAppMain.Documents, "prod.keys");
        if (!File.Exists(keys) || running) return;
        var keysTime = File.GetLastWriteTimeUtc(keys);
        var local = Dumps().Select(Path.GetFileName).ToList();
        var todo = Cloud.CloudOnlyGames(local, cloudFiles).Where(f => !GameAppUi.IsNup(f.Name)).Select(f =>
        {
            var rel = GameAppUi.IconRelativePath(f.Name);
            if (rel == null) return default;
            var icon = Path.Combine(GameAppMain.Documents, rel.Replace('/', Path.DirectorySeparatorChar));
            var none = Path.Combine(GameAppMain.Documents, GameAppUi.NoIconRelativePath(f.Name).Replace('/', Path.DirectorySeparatorChar));
            if (File.Exists(icon) || (File.Exists(none) && File.GetLastWriteTimeUtc(none) >= keysTime)) return default;
            return (File: f, Icon: icon, None: none);
        }).Where(x => x.File != null).ToList();
        if (todo.Count == 0 || Interlocked.CompareExchange(ref cloudIconsRunning, 1, 0) != 0) return;
        var nas = LoadNas();
        var password = NasPassword();
        var sources = cloudSources;
        System.Threading.Tasks.Task.Run(() =>
        {
            int got = 0;
            try
            {
                foreach (var (f, icon, none) in todo)
                {
                    byte[] jpeg = null; string why = null;
                    try
                    {
                        var source = sources.FirstOrDefault(s => s.Id == f.SourceId);
                        if (source == null) continue;
                        if (source.Kind == CloudKind.Nas)
                        {
                            if (string.IsNullOrEmpty(password)) continue;
                            jpeg = Nas.WithReadStream(nas, Nas.Password(nas.User, password), f.Remote, stream => GameIconsStream.Read(keys, stream));
                        }
                        else
                        {
                            var folder = ResolveFolder(source, out var whyNot);
                            if (folder == null) continue;
                            bool scoped = folder.StartAccessingSecurityScopedResource();
                            try
                            {
                                var path = Path.Combine(folder.Path, f.Remote ?? f.Name);
                                if (!File.Exists(path)) continue;   // only in iCloud (a placeholder): not downloaded for a cover
                                using var stream = File.OpenRead(path);
                                jpeg = GameIconsStream.Read(keys, stream);
                            }
                            finally { if (scoped) folder.StopAccessingSecurityScopedResource(); }
                        }
                    }
                    catch (Exception error) { why = (error.InnerException ?? error).Message; }
                    try
                    {
                        Directory.CreateDirectory(Path.GetDirectoryName(icon));
                        if (jpeg != null && jpeg.Length > 0) { File.WriteAllBytes(icon, jpeg); got++; }
                        else File.WriteAllText(none, (why ?? "no icon in the control data") + "\n");
                    }
                    catch (Exception) { }
                    Console.Error.WriteLine($"[gameapp] cloud icon of {f.Name}: {(jpeg != null ? jpeg.Length + " bytes" : why ?? "none")}");
                    if (got > 0 && got % 3 == 0) BeginInvokeOnMainThread(RefreshScreen);
                }
            }
            finally { Interlocked.Exchange(ref cloudIconsRunning, 0); }
            if (got > 0) BeginInvokeOnMainThread(RefreshScreen);
        });
    }

    private void ChooseNasFolder()
    {
        var nas = LoadNas();
        var password = NasPassword();
        if (!nas.IsComplete || string.IsNullOrEmpty(password)) { ConfigureNas(); return; }
        BrowseNas(nas, password, "");
    }

    /// One level of the NAS's folders as a sheet: use this one, make a new folder in it, go up, or go into one. The
    /// first listing also pins the server's key (trust on first use, as a test does).
    private void BrowseNas(NasConfig nas, string password, string at)
    {
        System.Threading.Tasks.Task.Run(() =>
        {
            List<string> folders = null; string error = null, seen = null;
            try { folders = Nas.Folders(nas, Nas.Password(nas.User, password), at, null, out seen); }
            catch (Exception e) { error = e.Message; }
            BeginInvokeOnMainThread(() =>
            {
                if (error != null) { Write("NAS: the folders could not be listed: " + error); var a = UIAlertController.Create("NAS", error, UIAlertControllerStyle.Alert); a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null)); PresentWhenFree(a); return; }
                if (seen != null && string.IsNullOrEmpty(LoadNas().HostKey)) { SaveNas(LoadNas() with { HostKey = seen }); Write($"NAS: connected; the server's key is pinned ({seen})"); }
                var here = string.IsNullOrEmpty(at) ? "your home folder" : at;
                var now = LoadNas().Folder;
                var sheet = UIAlertController.Create("Choose the NAS folder", $"In {here}" + (string.IsNullOrEmpty(now) ? "" : $"\nNow syncing: {now}")
                                                     + (folders.Count > 30 ? $"\n({folders.Count} folders: the first 30)" : ""), UIAlertControllerStyle.ActionSheet);
                sheet.AddAction(UIAlertAction.Create("Use this folder", UIAlertActionStyle.Default, _ => UseNasFolder(string.IsNullOrEmpty(at) ? "." : at)));
                sheet.AddAction(UIAlertAction.Create("New folder…", UIAlertActionStyle.Default, _ => NewNasFolder(nas, password, at)));
                if (!string.IsNullOrEmpty(at) && at != "/")
                {
                    var up = at.TrimEnd('/');
                    var cut = up.LastIndexOf('/');
                    var parent = cut > 0 ? up.Substring(0, cut) : cut == 0 ? "/" : "";
                    sheet.AddAction(UIAlertAction.Create("↑ Up", UIAlertActionStyle.Default, _ => BrowseNas(nas, password, parent)));
                }
                foreach (var f in folders.Take(30))
                {
                    var folder = f;
                    sheet.AddAction(UIAlertAction.Create(Path.GetFileName(f.TrimEnd('/')) + "  ›", UIAlertActionStyle.Default, _ => BrowseNas(nas, password, folder)));
                }
                sheet.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
                Theme.Anchor(sheet, null, View);
                PresentWhenFree(sheet);
            });
        });
    }

    private void UseNasFolder(string folder)
    {
        SaveNas(LoadNas() with { Folder = folder });
        cloudFiles = cloudFiles.Where(f => f.SourceId != NasId).ToList();
        Write($"NAS: the sync folder is {folder}");
        RefreshCloudPage();
        RefreshCloud(NasId);
    }

    /// A new folder inside `at` (asked by name), made on the NAS, then opened.
    private void NewNasFolder(NasConfig nas, string password, string at)
    {
        var ask = UIAlertController.Create("New folder", "In " + (string.IsNullOrEmpty(at) ? "your home folder" : at), UIAlertControllerStyle.Alert);
        ask.AddTextField(f => { f.Placeholder = "name (iBuilder)"; f.AutocapitalizationType = UITextAutocapitalizationType.None; f.AutocorrectionType = UITextAutocorrectionType.No; });
        ask.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, _ => BrowseNas(nas, password, at)));
        ask.AddAction(UIAlertAction.Create("Create", UIAlertActionStyle.Default, _ =>
        {
            var name = (ask.TextFields[0].Text ?? "").Trim().Trim('/');
            if (name.Length == 0) name = "iBuilder";
            if (name.Contains('/') || name == "." || name == "..") { Write("NAS: a folder name cannot hold / or be . or .."); BrowseNas(nas, password, at); return; }
            var path = string.IsNullOrEmpty(at) ? name : at.TrimEnd('/') + "/" + name;
            System.Threading.Tasks.Task.Run(() =>
            {
                string error = null, made = null;
                try { made = Nas.MakeFolder(LoadNas(), Nas.Password(nas.User, password), path, null); }
                catch (Exception e) { error = e.Message; }
                BeginInvokeOnMainThread(() =>
                {
                    if (error != null) { Write("NAS: the folder could not be made: " + error); var a = UIAlertController.Create("NAS", error, UIAlertControllerStyle.Alert); a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null)); PresentWhenFree(a); return; }
                    Write($"NAS: made {made}");
                    BrowseNas(nas, password, made);
                });
            });
        }));
        PresentWhenFree(ask);
    }

    // ---- downloads, uploads, removing a download ---------------------------------------------------------------

    private void DownloadSelected()
    {
        var entry = entries.FirstOrDefault(e => e.Name == selected);
        if (entry?.Cloud == null || downloads.ContainsKey(entry.Name)) return;
        var local = Dumps().Select(Path.GetFileName).ToList();
        var set = Cloud.DownloadSet(entry.Name, entry.Cloud.SourceId, cloudFiles, local);
        if (set.Count == 0) set = new List<CloudFile> { entry.Cloud };
        Download(set, GameAppMain.Documents);
    }

    /// Downloads `files` into `folder` one after another (each as .part, renamed at the end), the first file's name
    /// carrying the whole set's progress.
    private void Download(List<CloudFile> files, string folder)
    {
        if (files.Count == 0) return;
        long need = files.Sum(f => f.Size);
        if (need > 0 && FreeBytes() > 0 && need + (1L << 30) > FreeBytes())
        {
            var a = UIAlertController.Create("Not enough space", $"{GameAppUi.SizeText(need)} to download, {GameAppUi.SizeText(FreeBytes())} free. Free some space first.", UIAlertControllerStyle.Alert);
            a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
            PresentWhenFree(a);
            return;
        }
        var key = files[0].Name;
        downloads[key] = 0;
        Directory.CreateDirectory(folder);
        StartDownloadTicker();
        var nas = LoadNas();
        var password = NasPassword();
        var sources = cloudSources;
        Write($"\n=== download {key}" + (files.Count > 1 ? $" (+{files.Count - 1}: its update and DLC)" : "") + (need > 0 ? $", {GameAppUi.SizeText(need)}" : ""));
        nint task = UIApplication.SharedApplication.BeginBackgroundTask("download", null);
        UIApplication.SharedApplication.IdleTimerDisabled = true;
        System.Threading.Tasks.Task.Run(() =>
        {
            string error = null;
            long before = 0;
            try
            {
                foreach (var f in files)
                {
                    var dest = Path.Combine(folder, f.Name);
                    var source = sources.FirstOrDefault(s => s.Id == f.SourceId);
                    if (source == null) throw new InvalidOperationException("its source is gone");
                    long done0 = before;
                    Action<long, long> progress = (sent, total) =>
                    {
                        var all = need > 0 ? need : Math.Max(1, total);
                        var fraction = Math.Min(0.999, (done0 + sent) / (double) all);
                        BeginInvokeOnMainThread(() => { if (downloads.ContainsKey(key)) downloads[key] = fraction; });
                    };
                    if (source.Kind == CloudKind.Nas)
                        Nas.Download(nas, Nas.Password(nas.User, password), f.Remote, dest, progress, null);
                    else
                        CopyFromFolder(source, f, dest, progress);
                    before += f.Size;
                }
            }
            catch (Exception e) { error = e.Message; }
            BeginInvokeOnMainThread(() =>
            {
                downloads.Remove(key);
                UIApplication.SharedApplication.IdleTimerDisabled = false;
                UIApplication.SharedApplication.EndBackgroundTask(task);
                Write(error == null ? $"downloaded: {string.Join(", ", files.Select(f => f.Name))}" : $"download FAILED ({key}): {error}");
                if (error != null)
                {
                    var a = UIAlertController.Create("Download failed", error, UIAlertControllerStyle.Alert);
                    a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                    PresentWhenFree(a);
                }
                RefreshGames();
                RefreshScreen();
            });
        });
    }

    /// A file of a Files folder, copied here. The coordinated read makes the provider (iCloud Drive, Google Drive,
    /// OneDrive...) download it first when it is only in the cloud.
    private static void CopyFromFolder(CloudSource source, CloudFile f, string dest, Action<long, long> progress)
    {
        var folder = ResolveFolder(source, out var why);
        if (folder == null) throw new InvalidOperationException(why);
        bool scoped = folder.StartAccessingSecurityScopedResource();
        try
        {
            var url = folder.Append(f.Remote ?? f.Name, false);
            Exception failure = null;
            var coordinator = new NSFileCoordinator();
            coordinator.CoordinateRead(url, (NSFileCoordinatorReadingOptions) 0, out NSError error, readable =>
            {
                try { CopyWithProgress(readable.Path, dest, progress); }
                catch (Exception e) { failure = e; }
            });
            if (error != null) throw new InvalidOperationException(error.LocalizedDescription);
            if (failure != null) throw failure;
        }
        finally { if (scoped) folder.StopAccessingSecurityScopedResource(); }
    }

    private static void CopyWithProgress(string from, string dest, Action<long, long> progress)
    {
        var part = dest + ".part";
        long total = new FileInfo(from).Length, done = 0;
        var buffer = new byte[4 << 20];
        using (var input = File.OpenRead(from))
        using (var output = File.Create(part))
        {
            int n;
            var last = DateTime.MinValue;
            while ((n = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, n);
                done += n;
                if ((DateTime.Now - last).TotalMilliseconds > 300) { progress?.Invoke(done, total); last = DateTime.Now; }
            }
        }
        if (File.Exists(dest)) File.Delete(dest);
        File.Move(part, dest);
    }

    private void UploadTo(CloudSource target, string path, bool isApp)
    {
        if (target.Kind == CloudKind.Nas) { SendToNas(path, isApp ? "Exports" : "", offerDelete: false); return; }
        var name = Path.GetFileName(path);
        Write($"\n=== upload {name} to {target.Name}");
        nint task = UIApplication.SharedApplication.BeginBackgroundTask("upload", null);
        System.Threading.Tasks.Task.Run(() =>
        {
            string error = null;
            try
            {
                var folder = ResolveFolder(target, out var why);
                if (folder == null) throw new InvalidOperationException(why);
                bool scoped = folder.StartAccessingSecurityScopedResource();
                try
                {
                    var url = folder.Append(name, false);
                    Exception failure = null;
                    new NSFileCoordinator().CoordinateWrite(url, NSFileCoordinatorWritingOptions.ForReplacing, out NSError err, writable =>
                    {
                        try { CopyWithProgress(path, writable.Path, null); }
                        catch (Exception e) { failure = e; }
                    });
                    if (err != null) throw new InvalidOperationException(err.LocalizedDescription);
                    if (failure != null) throw failure;
                }
                finally { if (scoped) folder.StopAccessingSecurityScopedResource(); }
            }
            catch (Exception e) { error = e.Message; }
            BeginInvokeOnMainThread(() =>
            {
                UIApplication.SharedApplication.EndBackgroundTask(task);
                Write(error == null ? $"uploaded {name} to {target.Name} (the provider sends it to the cloud)" : $"upload FAILED: {error}");
                var a = UIAlertController.Create(error == null ? "Uploaded" : "Upload failed", error ?? $"{name} is in {target.Name}.", UIAlertControllerStyle.Alert);
                a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                PresentWhenFree(a);
                RefreshCloud(target.Id);
            });
        });
    }

    /// A local game that is also in the cloud: its dump (and its update and DLC that are in the cloud too) leave this
    /// iPhone; it shows greyed again, a tap brings it back. Its settings, saves and shader library stay.
    private void RemoveGameDownload(string game)
    {
        var names = Dumps().Select(Path.GetFileName).ToList();
        var set = new List<string> { game };
        var update = GameAppUi.UpdateFor(game, names);
        if (update != null) set.Add(update);
        set.AddRange(GameAppUi.DlcFor(game, names));
        var paths = set.Where(n => Cloud.CloudCopyOf(n, cloudFiles) != null).Select(n => Path.Combine(GameAppMain.Documents, n)).ToList();
        RemoveFiles(paths, true);
    }

    private void RemoveFiles(IReadOnlyList<string> paths, bool inCloud)
    {
        long bytes = paths.Sum(SafeLength);
        var a = UIAlertController.Create(inCloud ? "Remove the download?" : "Delete?",
            (inCloud ? "It stays in the cloud and shows greyed; a tap downloads it again. " : "It is not in the cloud: this cannot be undone. ")
            + $"Frees {GameAppUi.SizeText(bytes)}.", UIAlertControllerStyle.Alert);
        a.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
        a.AddAction(UIAlertAction.Create(inCloud ? "Remove" : "Delete", UIAlertActionStyle.Destructive, _ =>
        {
            foreach (var p in paths)
                try { File.Delete(p); Write($"removed from this iPhone: {Path.GetFileName(p)}" + (inCloud ? " (still in the cloud)" : "")); }
                catch (Exception e) { Write($"not removed: {Path.GetFileName(p)}: {e.Message}"); }
            // A game's folder under Exports goes with its last game file.
            foreach (var p in paths) GameAppUi.RemoveEmptyGameFolder(Path.Combine(GameAppMain.Documents, "Exports"), p);
            RefreshScreen();
        }));
        PresentWhenFree(a);
    }

    private void StartDownloadTicker()
    {
        if (downloadTicker != null) return;
        downloadTicker = new Timer(_ => BeginInvokeOnMainThread(() =>
        {
            if (downloads.Count == 0) { downloadTicker?.Dispose(); downloadTicker = null; return; }
            var facts = GatherFacts();
            carousel.SetItems(entries.Select(e => (e.Icon, GameAppUi.DisplayName(e.Name), IsCloudOnly(e.Name), CoverFraction(e.Name, facts))).ToList());
            RefreshSelection(facts);
            RefreshList(facts);
        }), null, TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(1));
    }
}

/// The build screen (design C, "Ambient — Game building"): the game, a large ring with how far the build is and the step
/// in its middle, the steps as chips (done with their time, now, next), the time left and the memory, the last lines of
/// the log (a tap opens it, back returns here), and Notify me. The main screen hands it a Model each second.
public sealed class GameBuildViewController : UIViewController
{
    public sealed record Model(string Name, UIImage Icon, string Meta, List<GameAppUi.BuildStep> Steps, int Current, double Fraction,
                               bool Running, bool Failed, string TimeLeft, string Clock, bool RamUnlocked, bool Extended, string MemoryLine,
                               List<(string Text, GameAppUi.LogKind Kind)> Live, bool Notify, string PrimaryTitle, bool PrimaryEnabled, UIColor Tint);

    public Action OnLog, OnPrimary, OnNotify;
    private readonly AmbientView ambient = new AmbientView();
    private readonly UIImageView icon = new UIImageView { ContentMode = UIViewContentMode.ScaleAspectFill, ClipsToBounds = true };
    private readonly UILabel name = new UILabel(), meta = new UILabel();
    private readonly UILabel stepOf = new UILabel(), percent = new UILabel(), stepName = new UILabel(), stepDetail = new UILabel();
    private readonly CAShapeLayer ringBack = new CAShapeLayer(), ring = new CAShapeLayer();
    private readonly UIView ringView = new UIView();
    private readonly UIStackView chips = new UIStackView { Axis = UILayoutConstraintAxis.Horizontal, Spacing = 8, Alignment = UIStackViewAlignment.Center };
    private readonly UILabel leftValue = new UILabel(), leftNote = new UILabel(), ramLine = new UILabel(), extLine = new UILabel(), memNote = new UILabel();
    private readonly UIStackView liveLines = new UIStackView { Axis = UILayoutConstraintAxis.Vertical, Spacing = 4 };
    private UIButton primary, notify;
    private Model last;

    public override UIInterfaceOrientationMask GetSupportedInterfaceOrientations() => Theme.IsPad ? UIInterfaceOrientationMask.All : UIInterfaceOrientationMask.Portrait;

    // The ring is 250 pt on a phone and larger on an iPad; the columns sit side by side in a wide window (ViewDidLayoutSubviews).
    private NSLayoutConstraint ringWidth, ringHeight, columnMax;
    private UIStackView body;
    private bool bodyWide;
    private nfloat ringSide = 250;

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        View.BackgroundColor = Theme.Ground;
        ambient.Frame = View.Bounds;
        ambient.AutoresizingMask = UIViewAutoresizing.FlexibleDimensions;
        View.AddSubview(ambient);

        var close = Theme.Round("chevron.down", "Back to the library");
        close.TouchUpInside += (_, __) => DismissViewController(true, null);
        icon.Layer.CornerRadius = 10;
        icon.TranslatesAutoresizingMaskIntoConstraints = false;
        icon.WidthAnchor.ConstraintEqualTo(38).Active = true;
        icon.HeightAnchor.ConstraintEqualTo(38).Active = true;
        name.Font = Theme.Font(16, UIFontWeight.Semibold); name.TextColor = Theme.Text; name.Lines = 1;
        meta.Font = Theme.Font(12, UIFontWeight.Regular); meta.TextColor = Theme.Hex(0xC3C9E2); meta.Lines = 1;
        var top = Theme.Row(12, close, icon, Theme.Column(1, name, meta), new UIView());

        // The ring: 250 pt, its track and its arc; the step in its middle.
        ringView.TranslatesAutoresizingMaskIntoConstraints = false;
        ringWidth = ringView.WidthAnchor.ConstraintEqualTo(250);
        ringHeight = ringView.HeightAnchor.ConstraintEqualTo(250);
        ringWidth.Active = ringHeight.Active = true;
        foreach (var l in new[] { ringBack, ring })
        {
            l.FillColor = UIColor.Clear.CGColor;
            l.LineWidth = 14;
            l.LineCap = CAShapeLayer.CapRound;
            l.Path = UIBezierPath.FromArc(new CGPoint(125, 125), 108, (nfloat) (-Math.PI / 2), (nfloat) (Math.PI * 1.5), true).CGPath;
            ringView.Layer.AddSublayer(l);
        }
        ringBack.StrokeColor = UIColor.FromWhiteAlpha(1f, 0.12f).CGColor;
        ringBack.FillColor = Theme.Hex(0x0F1324, 0.55f).CGColor;
        ring.StrokeColor = UIColor.White.CGColor;
        ring.StrokeEnd = 0;
        stepOf.Font = Theme.Font(12, UIFontWeight.Semibold); stepOf.TextColor = Theme.Hex(0xC3C9E2);
        percent.Font = Theme.Rounded(58, UIFontWeight.Bold); percent.TextColor = Theme.Text;
        stepName.Font = Theme.Font(15, UIFontWeight.Semibold); stepName.TextColor = Theme.Text;
        stepDetail.Font = Theme.Mono(12); stepDetail.TextColor = Theme.Hex(0xC3C9E2);
        foreach (var l in new[] { stepOf, percent, stepName, stepDetail }) { l.TextAlignment = UITextAlignment.Center; l.AdjustsFontSizeToFitWidth = true; l.MinimumScaleFactor = 0.6f; }
        var middle = Theme.Column(2, stepOf, percent, stepName, stepDetail);
        middle.Alignment = UIStackViewAlignment.Center;
        middle.TranslatesAutoresizingMaskIntoConstraints = false;
        ringView.AddSubview(middle);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            middle.CenterXAnchor.ConstraintEqualTo(ringView.CenterXAnchor), middle.CenterYAnchor.ConstraintEqualTo(ringView.CenterYAnchor),
            middle.WidthAnchor.ConstraintLessThanOrEqualTo(180),
        });
        var ringRow = Theme.Row(0, new UIView(), ringView, new UIView());
        ringRow.Distribution = UIStackViewDistribution.EqualCentering;
        var chipsRow = Theme.Row(0, new UIView(), chips, new UIView());
        chipsRow.Distribution = UIStackViewDistribution.EqualCentering;

        // Time left, and memory.
        UIView Card(UIView content) => Theme.Wrap(content, 22, 14);
        leftValue.Font = Theme.Rounded(22, UIFontWeight.Bold); leftValue.TextColor = Theme.Text; leftValue.AdjustsFontSizeToFitWidth = true; leftValue.MinimumScaleFactor = 0.6f;
        leftNote.Font = Theme.Font(12, UIFontWeight.Regular); leftNote.TextColor = Theme.Hex(0xC3C9E2);
        var timeCard = Card(Theme.Column(4, Theme.Label("Time left", Theme.Font(12, UIFontWeight.Regular), Theme.Hex(0xC3C9E2)), leftValue, leftNote));
        foreach (var l in new[] { ramLine, extLine }) { l.Font = Theme.Font(14, UIFontWeight.Semibold); l.TextColor = Theme.Text; l.AdjustsFontSizeToFitWidth = true; l.MinimumScaleFactor = 0.7f; }
        memNote.Font = Theme.Font(12, UIFontWeight.Regular); memNote.TextColor = Theme.Hex(0xC3C9E2);
        var memCard = Card(Theme.Column(4, Theme.Label("Memory", Theme.Font(12, UIFontWeight.Regular), Theme.Hex(0xC3C9E2)), ramLine, extLine, memNote));
        var cards = new UIStackView(new[] { timeCard, memCard }) { Axis = UILayoutConstraintAxis.Horizontal, Distribution = UIStackViewDistribution.FillEqually, Spacing = 10 };

        // The live log: its last lines; the whole card opens the log.
        var liveTitle = Theme.Row(6, Theme.Label("Live log", Theme.Font(13, UIFontWeight.Semibold), Theme.Text), new UIView(),
                                  Theme.Label("Open", Theme.Font(13, UIFontWeight.Medium), Theme.Hex(0xC3C9E2)), Theme.Symbol("chevron.right", 12, Theme.Hex(0xC3C9E2)));
        var liveColumn = Theme.Column(8, liveTitle, liveLines);
        liveColumn.UserInteractionEnabled = false;
        var liveButton = new UIButton(UIButtonType.System) { BackgroundColor = Theme.Hex(0x080A14, 0.6f), AccessibilityLabel = "Open the build log", AccessibilityIdentifier = "build-live-log" };
        liveButton.Layer.CornerRadius = 22;
        liveButton.Layer.BorderWidth = 1;
        liveButton.Layer.BorderColor = UIColor.FromWhiteAlpha(1f, 0.12f).CGColor;
        liveColumn.TranslatesAutoresizingMaskIntoConstraints = false;
        liveButton.AddSubview(liveColumn);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            liveColumn.TopAnchor.ConstraintEqualTo(liveButton.TopAnchor, 14), liveColumn.BottomAnchor.ConstraintEqualTo(liveButton.BottomAnchor, -14),
            liveColumn.LeadingAnchor.ConstraintEqualTo(liveButton.LeadingAnchor, 14), liveColumn.TrailingAnchor.ConstraintEqualTo(liveButton.TrailingAnchor, -14),
        });
        liveButton.TouchUpInside += (_, __) => OnLog?.Invoke();

        notify = Theme.Pill("Notify me", "bell", false, 56);
        notify.AccessibilityIdentifier = "build-notify";
        notify.TouchUpInside += (_, __) => OnNotify?.Invoke();
        primary = Theme.Pill("Play", "play.fill", true, 56);
        primary.AccessibilityIdentifier = "build-primary";
        primary.TouchUpInside += (_, __) => OnPrimary?.Invoke();
        var buttons = new UIStackView(new UIView[] { notify, primary }) { Axis = UILayoutConstraintAxis.Horizontal, Distribution = UIStackViewDistribution.FillEqually, Spacing = 10 };
        var buttonsWidth = buttons.WidthAnchor.ConstraintEqualTo(View.SafeAreaLayoutGuide.WidthAnchor, 1, -36);   // 18 pt each side; an iPad's are 560 wide at most
        buttonsWidth.Priority = 999;

        // The ring and its steps, then the time, the memory and the live log: under each other on a phone and in a narrow window, side by
        // side in a wide one (ViewDidLayoutSubviews). The two stacks are one column with the same 18 pt gaps until they are separated.
        var ringStack = Theme.Column(18, ringRow, chipsRow);
        var infoStack = Theme.Column(18, cards, liveButton);
        body = new UIStackView(new UIView[] { ringStack, infoStack }) { Axis = UILayoutConstraintAxis.Vertical, Spacing = 18 };
        var column = Theme.Column(18, top, body);
        var scroll = new UIScrollView { AlwaysBounceVertical = true, ShowsVerticalScrollIndicator = false };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.TranslatesAutoresizingMaskIntoConstraints = false;
        buttons.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        View.AddSubview(scroll);
        View.AddSubview(buttons);
        var g = View.SafeAreaLayoutGuide;
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            scroll.TopAnchor.ConstraintEqualTo(g.TopAnchor), scroll.LeadingAnchor.ConstraintEqualTo(g.LeadingAnchor),
            scroll.TrailingAnchor.ConstraintEqualTo(g.TrailingAnchor), scroll.BottomAnchor.ConstraintEqualTo(buttons.TopAnchor, -10),
            buttons.CenterXAnchor.ConstraintEqualTo(g.CenterXAnchor), buttons.BottomAnchor.ConstraintEqualTo(g.BottomAnchor, -8),
            buttons.WidthAnchor.ConstraintLessThanOrEqualTo(560), buttonsWidth,
        });
        columnMax = Theme.PinColumn(scroll, column, 8, 12, 18, 560);
        if (last != null) Apply(last);
        Theme.Hover(View);
    }

    /// A wide window (an iPad turned sideways, a wide Stage Manager window): the ring on the left, the time, memory and live log on the
    /// right; a tall or narrow one keeps the phone's single column, centred. The ring grows with the room.
    public override void ViewDidLayoutSubviews()
    {
        base.ViewDidLayoutSubviews();
        var w = View.SafeAreaLayoutGuide.LayoutFrame.Width;
        bool wide = w >= 900 && View.Bounds.Width > View.Bounds.Height;   // sideways only: upright, the column is simply centred
        if (wide != bodyWide)
        {
            bodyWide = wide;
            body.Axis = wide ? UILayoutConstraintAxis.Horizontal : UILayoutConstraintAxis.Vertical;
            body.Distribution = wide ? UIStackViewDistribution.FillEqually : UIStackViewDistribution.Fill;
            body.Alignment = wide ? UIStackViewAlignment.Center : UIStackViewAlignment.Fill;
            body.Spacing = wide ? 36 : 18;
        }
        columnMax.Constant = wide ? 1040 : 560;
        var side = (nfloat) (w >= 700 ? Math.Min(340.0, w * (wide ? 0.30 : 0.45)) : 250.0);
        if (side != ringSide)
        {
            ringSide = side;
            ringWidth.Constant = ringHeight.Constant = side;
        }
        // the arc is drawn for the ring's own size (a phone's is the 250 pt one, radius 108)
        var c = new CGPoint(ringSide / 2, ringSide / 2);
        var path = UIBezierPath.FromArc(c, ringSide * 0.432f, (nfloat) (-Math.PI / 2), (nfloat) (Math.PI * 1.5), true).CGPath;
        CATransaction.Begin();
        CATransaction.DisableActions = true;   // a resized ring is redrawn, not morphed
        foreach (var l in new[] { ringBack, ring }) { l.Path = path; l.Frame = new CGRect(0, 0, ringSide, ringSide); }
        CATransaction.Commit();
    }

    public void Update(Model m)
    {
        last = m;
        if (IsViewLoaded) Apply(m);
    }

    private static UIView Chip(GameAppUi.BuildStep step)
    {
        var short_ = step.Name switch
        {
            "Read the game" => "Read", "Prepare the profile" => "Profile", "Rewrite the code for iPhone" => "Rewrite",
            "Translate the shaders" => "Shaders", "Sign the game" => "Sign", _ => step.Name,
        };
        var text = step.State == GameAppUi.StepState.Done && !string.IsNullOrEmpty(step.Time) ? short_ + " " + step.Time : short_;
        var l = new UILabel { Text = text, Font = Theme.Font(12, step.State == GameAppUi.StepState.Now ? UIFontWeight.Semibold : UIFontWeight.Regular), TextAlignment = UITextAlignment.Center };
        var v = new UIView();
        v.Layer.CornerRadius = 15;
        if (step.State == GameAppUi.StepState.Now) { v.BackgroundColor = UIColor.White; l.TextColor = Theme.Ground; }
        else if (step.State == GameAppUi.StepState.Done) { v.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.14f); l.TextColor = Theme.Text; }
        else { v.Layer.BorderWidth = 1; v.Layer.BorderColor = UIColor.FromWhiteAlpha(1f, 0.22f).CGColor; l.TextColor = Theme.Hex(0xC3C9E2); }
        l.TranslatesAutoresizingMaskIntoConstraints = false;
        v.AddSubview(l);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            v.HeightAnchor.ConstraintEqualTo(30), l.CenterYAnchor.ConstraintEqualTo(v.CenterYAnchor),
            l.LeadingAnchor.ConstraintEqualTo(v.LeadingAnchor, 10), l.TrailingAnchor.ConstraintEqualTo(v.TrailingAnchor, -10),
        });
        v.IsAccessibilityElement = true;
        v.AccessibilityLabel = $"{step.Name}: {(step.State == GameAppUi.StepState.Done ? "done " + step.Time : step.State == GameAppUi.StepState.Now ? "now" : "to come")}";
        return v;
    }

    private void Apply(Model m)
    {
        ambient.SetTint(m.Tint, true);
        icon.Image = m.Icon;
        icon.BackgroundColor = m.Icon == null ? Theme.ColorFor(m.Name) : UIColor.Clear;
        name.Text = m.Name;
        meta.Text = m.Meta;
        var step = m.Steps[Math.Max(0, Math.Min(m.Current, m.Steps.Count - 1))];
        stepOf.Text = m.Running ? $"STEP {m.Current + 1} OF {m.Steps.Count}" : m.Failed ? "STOPPED" : "BUILT";
        percent.Text = $"{Math.Round(m.Fraction * 100):0}%";
        stepName.Text = m.Running ? step.Name switch { "Translate the shaders" => "Shaders", "Rewrite the code for iPhone" => "Rewrite", "Prepare the profile" => "Profile", "Read the game" => "Reading", "Sign the game" => "Signing", _ => step.Name }
                                  : m.Failed ? step.Name : "Ready to play";
        stepDetail.Text = m.Running ? step.Detail : "";
        stepDetail.Hidden = string.IsNullOrEmpty(stepDetail.Text);
        ring.StrokeEnd = (nfloat) Math.Max(0.01, Math.Min(1, m.Fraction));
        ring.StrokeColor = (m.Failed ? Theme.Error : UIColor.White).CGColor;

        // Chips: the step before the current one, the current one, and the next ones that fit (four at most).
        foreach (var v in chips.ArrangedSubviews.ToArray()) { chips.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        int from = Math.Max(0, Math.Min(m.Current - 1, m.Steps.Count - 4));
        for (int i = from; i < Math.Min(m.Steps.Count, from + 4); i++) chips.AddArrangedSubview(Chip(m.Steps[i]));

        leftValue.Text = m.TimeLeft;
        leftNote.Text = m.Clock;
        ramLine.AttributedText = Check(m.RamUnlocked, "RAM unlocked", "RAM not unlocked");
        extLine.AttributedText = Check(m.Extended, "Extended", "Not extended");
        memNote.Text = m.MemoryLine;
        memNote.Hidden = string.IsNullOrEmpty(m.MemoryLine);

        foreach (var v in liveLines.ArrangedSubviews.ToArray()) { liveLines.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        if (m.Live.Count == 0) liveLines.AddArrangedSubview(Theme.Label("Nothing yet.", Theme.Mono(11.5f), Theme.Hex(0xC3C9E2)));
        foreach (var (text, kind) in m.Live)
        {
            var l = Theme.Label(text, Theme.Mono(11.5f, kind == GameAppUi.LogKind.Error ? UIFontWeight.Semibold : UIFontWeight.Regular),
                                kind == GameAppUi.LogKind.Error ? Theme.Error : kind == GameAppUi.LogKind.Warning ? Theme.Warn : Theme.Hex(0xC3C9E2), 2);
            l.LineBreakMode = UILineBreakMode.TailTruncation;
            liveLines.AddArrangedSubview(l);
        }

        var nc = notify.Configuration;
        nc.AttributedTitle = new NSAttributedString(m.Notify ? "Will notify" : "Notify me", new UIStringAttributes { Font = Theme.Font(16, UIFontWeight.Semibold) });
        nc.Image = UIImage.GetSystemImage(m.Notify ? "bell.fill" : "bell");
        notify.Configuration = nc;
        notify.Hidden = !m.Running;
        var pc = primary.Configuration;
        pc.AttributedTitle = new NSAttributedString(m.PrimaryTitle, new UIStringAttributes { Font = Theme.Font(16, UIFontWeight.Semibold) });
        pc.Image = m.Running || m.Failed ? null : UIImage.GetSystemImage("play.fill");
        primary.Configuration = pc;
        primary.Enabled = m.PrimaryEnabled;
    }

    private static NSAttributedString Check(bool ok, string yes, string no)
    {
        var s = new NSMutableAttributedString();
        s.Append(new NSAttributedString(ok ? "✓ " : "✗ ", new UIStringAttributes { Font = Theme.Font(14, UIFontWeight.Heavy), ForegroundColor = ok ? Theme.Ok : Theme.Warn }));
        s.Append(new NSAttributedString(ok ? yes : no, new UIStringAttributes { Font = Theme.Font(14, UIFontWeight.Semibold), ForegroundColor = Theme.Text }));
        return s;
    }
}
