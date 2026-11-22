// gameapp/GameAppSavesUi.cs: the saves sheet (UIKit), over GameAppSaves.cs's logic (which the Mac tests run).
// Inspired by what save managers offer (Cartouche's copies, JKSV's backups), written for this app.

using System;
using System.Linq;

// ---- the saves sheet: the game's save now, its copies, restore / share / delete, JKSV in and out ----------------------
public sealed class GameAppSavesViewController : UIKit.UIViewController
{
    private readonly string documents, titleId, gameName;
    private readonly Action<string> log;
    private readonly UIKit.UIStackView rows = new UIKit.UIStackView { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 10 };
    private readonly UIKit.UILabel current = new UIKit.UILabel { Lines = 0 };

    /// Set by the screen when a NAS is set up: "Send to the NAS" on a copy.
    public Action<string> SendToNas { get; set; }

    public GameAppSavesViewController(string documents, string titleId, string gameName, Action<string> log)
    {
        this.documents = documents; this.titleId = titleId; this.gameName = gameName; this.log = log;
    }

    private static UIKit.UILabel Text(string text, UIKit.UIFontTextStyle style, UIKit.UIColor color, bool bold = false)
    {
        var font = UIKit.UIFont.GetPreferredFontForTextStyle(style);
        if (bold) font = UIKit.UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(UIKit.UIFontDescriptorSymbolicTraits.Bold), font.PointSize);
        return new UIKit.UILabel { Text = text, Font = font, TextColor = color, Lines = 0, AdjustsFontForContentSizeCategory = true };
    }

    private UIKit.UIButton Button(string title, string symbol, string id, Action tapped, bool filled = false)
    {
        var b = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
        var c = filled ? UIKit.UIButtonConfiguration.FilledButtonConfiguration : UIKit.UIButtonConfiguration.TintedButtonConfiguration;
        c.Title = title; c.Image = UIKit.UIImage.GetSystemImage(symbol); c.ImagePadding = 8;
        b.Configuration = c; b.AccessibilityIdentifier = id;
        b.TouchUpInside += (_, __) => tapped();
        return b;
    }

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        View.BackgroundColor = UIKit.UIColor.SystemBackground;
        var done = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
        done.SetTitle("Done", UIKit.UIControlState.Normal);
        done.AccessibilityIdentifier = "saves-done";
        done.TouchUpInside += (_, __) => DismissViewController(true, null);
        var header = new UIKit.UIStackView(new UIKit.UIView[] { Text("Saves", UIKit.UIFontTextStyle.Title3, UIKit.UIColor.Label, bold: true), new UIKit.UIView(), done })
        { Axis = UIKit.UILayoutConstraintAxis.Horizontal, Alignment = UIKit.UIStackViewAlignment.Center };

        var actions = new UIKit.UIStackView(new UIKit.UIView[]
        {
            Button("Make a copy now", "doc.on.doc", "saves-copy", MakeCopy, filled: true),
            Button("Import a save (JKSV or a copy)…", "square.and.arrow.down", "saves-import", Import),
            Button("Export for JKSV (Switch)…", "square.and.arrow.up", "saves-export-jksv", ExportJksv),
        }) { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 8 };
        var note = Text($"A copy is made by itself before each launch; the {GameSaves.KeepAuto} newest are kept, your own copies stay until you delete them. "
                        + "Copies are zips in Files, in this app's SaveBackups folder. JKSV is the save manager on a Switch: its backups come in and go out as they are.",
                        UIKit.UIFontTextStyle.Footnote, UIKit.UIColor.SecondaryLabel);
        var stack = new UIKit.UIStackView(new UIKit.UIView[] { header, current, actions, Text("Copies", UIKit.UIFontTextStyle.Headline, UIKit.UIColor.Label), rows, note })
        { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 16, TranslatesAutoresizingMaskIntoConstraints = false };
        var scroll = new UIKit.UIScrollView { TranslatesAutoresizingMaskIntoConstraints = false };
        scroll.AddSubview(stack);
        View.AddSubview(scroll);
        var guide = View.SafeAreaLayoutGuide;
        UIKit.NSLayoutConstraint.ActivateConstraints(new[]
        {
            scroll.TopAnchor.ConstraintEqualTo(View.TopAnchor), scroll.BottomAnchor.ConstraintEqualTo(guide.BottomAnchor),
            scroll.LeadingAnchor.ConstraintEqualTo(guide.LeadingAnchor), scroll.TrailingAnchor.ConstraintEqualTo(guide.TrailingAnchor),
            stack.TopAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.TopAnchor, 20), stack.BottomAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.BottomAnchor, -20),
            stack.LeadingAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.LeadingAnchor, 20), stack.TrailingAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.TrailingAnchor, -20),
        });
        Reload();
    }

    private void Reload()
    {
        var (bytes, changed) = GameSaves.Current(documents, titleId);
        current.AttributedText = null;
        current.Font = UIKit.UIFont.GetPreferredFontForTextStyle(UIKit.UIFontTextStyle.Subheadline);
        current.TextColor = UIKit.UIColor.SecondaryLabel;
        current.Text = changed == null ? "No save yet: the game has not saved." : $"Current save: {GameAppUi.SizeText(bytes)}, last written {changed.Value:g}";
        current.AccessibilityIdentifier = "saves-current";
        foreach (var v in rows.ArrangedSubviews) { rows.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        var list = GameSaves.List(documents, titleId, gameName);
        if (list.Count == 0) rows.AddArrangedSubview(Text("No copy yet.", UIKit.UIFontTextStyle.Subheadline, UIKit.UIColor.SecondaryLabel));
        int i = 0;
        foreach (var c in list)
        {
            var copy = c;
            var b = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
            var cfg = UIKit.UIButtonConfiguration.GrayButtonConfiguration;
            cfg.Title = copy.When.ToString("g");
            cfg.Subtitle = $"{GameSaves.KindLabel(copy.Kind)} · {GameAppUi.SizeText(copy.Size)}";
            cfg.TitleAlignment = UIKit.UIButtonConfigurationTitleAlignment.Leading;
            cfg.Image = UIKit.UIImage.GetSystemImage(copy.Kind == "auto" ? "clock.arrow.circlepath" : "doc.zipper");
            cfg.ImagePadding = 10;
            b.Configuration = cfg;
            b.AccessibilityIdentifier = "saves-copy-" + i++;
            b.TouchUpInside += (_, __) => CopyActions(copy, b);
            rows.AddArrangedSubview(b);
        }
    }

    private void Say(string title, string message)
    {
        var a = UIKit.UIAlertController.Create(title, message, UIKit.UIAlertControllerStyle.Alert);
        a.AddAction(UIKit.UIAlertAction.Create("OK", UIKit.UIAlertActionStyle.Cancel, null));
        PresentViewController(a, true, null);
    }

    private void MakeCopy()
    {
        try
        {
            var p = GameSaves.Create(documents, titleId, gameName, "manual", DateTime.Now);
            log?.Invoke(p == null ? "saves: no save to copy yet" : $"saves: copy made, {System.IO.Path.GetFileName(p)}");
            if (p == null) Say("No save yet", "The game has not saved anything yet.");
        }
        catch (Exception e) { Say("The copy failed", e.Message); }
        Reload();
    }

    private void CopyActions(GameSaves.Copy copy, UIKit.UIView source)
    {
        var sheet = UIKit.UIAlertController.Create(copy.When.ToString("g"), GameSaves.KindLabel(copy.Kind), UIKit.UIAlertControllerStyle.ActionSheet);
        sheet.AddAction(UIKit.UIAlertAction.Create("Restore this copy…", UIKit.UIAlertActionStyle.Default, _ => ConfirmRestore(copy.Path, "before-restore",
            "The game's save becomes this copy. The save it replaces is kept as a copy first.")));
        sheet.AddAction(UIKit.UIAlertAction.Create("Share…", UIKit.UIAlertActionStyle.Default, _ => Share(copy.Path, source)));
        if (SendToNas != null)
            sheet.AddAction(UIKit.UIAlertAction.Create("Send to the NAS", UIKit.UIAlertActionStyle.Default, _ => SendToNas(copy.Path)));
        sheet.AddAction(UIKit.UIAlertAction.Create("Delete", UIKit.UIAlertActionStyle.Destructive, _ =>
        {
            try { System.IO.File.Delete(copy.Path); log?.Invoke($"saves: copy deleted, {System.IO.Path.GetFileName(copy.Path)}"); } catch (Exception e) { Say("Not deleted", e.Message); }
            Reload();
        }));
        sheet.AddAction(UIKit.UIAlertAction.Create("Cancel", UIKit.UIAlertActionStyle.Cancel, null));
        Theme.Anchor(sheet, source, View);
        PresentViewController(sheet, true, null);
    }

    private void ConfirmRestore(string zip, string beforeKind, string message)
    {
        var a = UIKit.UIAlertController.Create("Replace the save?", message, UIKit.UIAlertControllerStyle.Alert);
        a.AddAction(UIKit.UIAlertAction.Create("Cancel", UIKit.UIAlertActionStyle.Cancel, null));
        a.AddAction(UIKit.UIAlertAction.Create("Replace", UIKit.UIAlertActionStyle.Destructive, _ =>
        {
            try
            {
                var kept = GameSaves.Restore(documents, titleId, gameName, zip, beforeKind, DateTime.Now);
                log?.Invoke($"saves: {System.IO.Path.GetFileName(zip)} put in place" + (kept != null ? $"; the replaced save kept as {System.IO.Path.GetFileName(kept)}" : ""));
            }
            catch (Exception e) { log?.Invoke("saves: restore failed: " + e.Message); Say("Not restored", e.Message); }
            Reload();
        }));
        PresentWhenFree(a);
    }

    /// Presented once the document picker has gone: an alert presented while it is still going away is dropped by
    /// UIKit without a word (the certificate import's "a tap does nothing", docs/DECISIONS.md trap 56).
    private void PresentWhenFree(UIKit.UIViewController vc, int tries = 20)
    {
        if (PresentedViewController == null) { PresentViewController(vc, true, null); return; }
        if (tries > 0) { Foundation.NSTimer.CreateScheduledTimer(0.15, _ => PresentWhenFree(vc, tries - 1)); return; }
        PresentedViewController.PresentViewController(vc, true, null);
    }

    private void Share(string path, UIKit.UIView source)
    {
        var s = new UIKit.UIActivityViewController(new Foundation.NSObject[] { Foundation.NSUrl.FromFilename(path) }, null);
        Theme.Anchor(s, source, View);
        PresentViewController(s, true, null);
    }

    private void ExportJksv()
    {
        try
        {
            var p = GameSaves.ExportJksv(documents, titleId, gameName, DateTime.Now);
            if (p == null) { Say("No save yet", "The game has not saved anything yet."); return; }
            log?.Invoke($"saves: JKSV zip written, {System.IO.Path.GetFileName(p)}");
            Share(p, View);
        }
        catch (Exception e) { Say("Not exported", e.Message); }
    }

    // Import mode (a copy into tmp/): Open mode's taps did nothing in a FlareStore install (docs/DECISIONS.md trap 56).
    private void Import()
    {
        var picker = new UIKit.UIDocumentPickerViewController(new[] { "public.zip-archive", "public.data", "public.item" }, UIKit.UIDocumentPickerMode.Import);
        picker.Delegate = new PickerDelegate(url => BeginInvokeOnMainThread(() =>
        {
            var name = url?.LastPathComponent ?? "";
            if (!name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)) { Say("Not a zip", $"{name} is not a .zip: pick a JKSV backup or one of these copies."); return; }
            // The picker's copy is ours: kept in tmp/ until the import, then deleted.
            var local = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "save-import-" + Guid.NewGuid().ToString("N") + ".zip");
            try { System.IO.File.Copy(url.Path, local, true); try { System.IO.File.Delete(url.Path); } catch { } }
            catch (Exception e) { Say("Not read", e.Message); return; }
            ConfirmRestore(local, "before-import", $"The game's save becomes {name}. The save it replaces is kept as a copy first.");
        }));
        PresentViewController(picker, true, null);
    }

    private sealed class PickerDelegate : Foundation.NSObject, UIKit.IUIDocumentPickerDelegate
    {
        private readonly Action<Foundation.NSUrl> picked;
        public PickerDelegate(Action<Foundation.NSUrl> picked) => this.picked = picked;
        public void DidPickDocument(UIKit.UIDocumentPickerViewController controller, Foundation.NSUrl url) => picked(url);
        public void DidPickDocumentAtUrls(UIKit.UIDocumentPickerViewController controller, Foundation.NSUrl[] urls)
        {
            if (urls != null && urls.Length > 0) picked(urls[0]);
        }
    }
}
