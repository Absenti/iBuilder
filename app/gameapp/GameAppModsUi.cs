// gameapp/GameAppModsUi.cs: the "Mods & cheats" sheet (UIKit), over GameAppMods.cs's logic (which the Mac tests run).
// A switch per mod, per .pchtxt patch block and per cheat; Add from Files (a .zip or a folder); delete. It changes files only:
// the build chain applies them (gameapp/mods.py), and the sheet says which kind needs a rebuild.

using System;
using System.IO;
using System.Linq;

public sealed class GameAppModsViewController : UIKit.UIViewController
{
    private readonly string documents, titleId, gameName;
    private readonly bool built;
    private readonly Action<string> log;
    private readonly UIKit.UIStackView rows = new UIKit.UIStackView { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 14 };
    private readonly UIKit.UILabel note = new UIKit.UILabel { Lines = 0 };

    /// `built`: the game has a build on this iPhone (an exefs patch change then needs a rebuild; before that it is just applied by the first one).
    public GameAppModsViewController(string documents, string titleId, string gameName, bool built, Action<string> log)
    {
        this.documents = documents; this.titleId = titleId; this.gameName = gameName; this.built = built; this.log = log;
    }

    private static UIKit.UILabel Text(string text, UIKit.UIFontTextStyle style, UIKit.UIColor color, bool bold = false)
    {
        var font = UIKit.UIFont.GetPreferredFontForTextStyle(style);
        if (bold) font = UIKit.UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(UIKit.UIFontDescriptorSymbolicTraits.Bold), font.PointSize);
        return new UIKit.UILabel { Text = text, Font = font, TextColor = color, Lines = 0, AdjustsFontForContentSizeCategory = true };
    }

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        View.BackgroundColor = UIKit.UIColor.SystemBackground;
        var done = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
        done.SetTitle("Done", UIKit.UIControlState.Normal);
        done.AccessibilityIdentifier = "mods-done";
        done.TouchUpInside += (_, __) => DismissViewController(true, null);
        var header = new UIKit.UIStackView(new UIKit.UIView[] { Text("Mods & cheats", UIKit.UIFontTextStyle.Title3, UIKit.UIColor.Label, bold: true), new UIKit.UIView(), done })
        { Axis = UIKit.UILayoutConstraintAxis.Horizontal, Alignment = UIKit.UIStackViewAlignment.Center };

        var add = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
        var cfg = UIKit.UIButtonConfiguration.FilledButtonConfiguration;
        cfg.Title = "Add from Files…"; cfg.Image = UIKit.UIImage.GetSystemImage("plus.square.on.square"); cfg.ImagePadding = 8;
        add.Configuration = cfg; add.AccessibilityIdentifier = "mods-add";
        add.TouchUpInside += (_, __) => ChooseSource(add);

        var help = Text("Mods in the Atmosphere layout (atmosphere/contents/<title id>/romfs, exefs, cheats; Ryujinx and yuzu mod folders; .pchtxt and .ips patches), "
                        + "as a .zip or a folder. Only what matches this game is kept. A 60 fps mod is an exefs patch: it usually goes with the game's FPS setting \"60\" "
                        + "in its settings. Patches of the code need a rebuild; RomFS files and cheats apply at the next start. Cheats can also be switched from the in-game menu.",
                        UIKit.UIFontTextStyle.Footnote, UIKit.UIColor.SecondaryLabel);
        var stack = new UIKit.UIStackView(new UIKit.UIView[] { header, add, note, rows, help })
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

    private UIKit.UIView SwitchRow(string title, string subtitle, bool on, string id, Action<bool> set, bool nested = false)
    {
        var sw = new UIKit.UISwitch { On = on, AccessibilityIdentifier = id };
        sw.ValueChanged += (_, __) => { set(sw.On); Reload(); };
        var labels = new UIKit.UIStackView(subtitle == null ? new UIKit.UIView[] { Text(title, nested ? UIKit.UIFontTextStyle.Subheadline : UIKit.UIFontTextStyle.Headline, UIKit.UIColor.Label) }
            : new UIKit.UIView[] { Text(title, nested ? UIKit.UIFontTextStyle.Subheadline : UIKit.UIFontTextStyle.Headline, UIKit.UIColor.Label), Text(subtitle, UIKit.UIFontTextStyle.Footnote, UIKit.UIColor.SecondaryLabel) })
        { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 2 };
        var row = new UIKit.UIStackView(new UIKit.UIView[] { labels, sw }) { Axis = UIKit.UILayoutConstraintAxis.Horizontal, Spacing = 12, Alignment = UIKit.UIStackViewAlignment.Center };
        if (nested) row.LayoutMargins = new UIKit.UIEdgeInsets(0, 18, 0, 0);
        row.LayoutMarginsRelativeArrangement = nested;
        return row;
    }

    private void Reload()
    {
        foreach (var v in rows.ArrangedSubviews) { rows.RemoveArrangedSubview(v); v.RemoveFromSuperview(); }
        var list = GameMods.List(documents, titleId);
        var changed = GameMods.ExefsChangedSinceBuild(documents, titleId);
        note.Font = UIKit.UIFont.GetPreferredFontForTextStyle(UIKit.UIFontTextStyle.Subheadline);
        note.AccessibilityIdentifier = "mods-note";
        if (changed)
        {
            note.TextColor = UIKit.UIColor.SystemOrange;
            note.Text = built ? "Rebuild to apply: the patches of the game's code changed since its last build (game settings, Rebuild from scratch)."
                              : "The patches of the game's code are applied when the game is built.";
        }
        else
        {
            note.TextColor = UIKit.UIColor.SecondaryLabel;
            note.Text = list.Count == 0 ? "No mods yet for " + gameName + "." : "Nothing to rebuild: the game was built with these patches.";
        }
        int i = 0;
        foreach (var m in list)
        {
            var mod = m; int mi = i++;
            var parts = new System.Collections.Generic.List<string>();
            if (mod.RomfsFiles > 0) parts.Add($"{mod.RomfsFiles} RomFS file(s)");
            if (mod.ExefsFiles > 0) parts.Add($"{mod.ExefsFiles} code patch file(s)");
            if (mod.CheatFiles > 0) parts.Add($"{mod.Cheats.Count} cheat(s)");
            parts.Add(GameAppUi.SizeText(mod.Bytes));
            var card = new UIKit.UIStackView { Axis = UIKit.UILayoutConstraintAxis.Vertical, Spacing = 8 };
            card.AddArrangedSubview(SwitchRow(mod.Name, string.Join(" · ", parts), mod.Enabled, "mod-switch-" + mi,
                on => { GameMods.SetMod(documents, titleId, mod.Name, on); log?.Invoke($"mods: {mod.Name} {(on ? "on" : "off")}"); }));
            int pj = 0;
            foreach (var p in mod.Patches)
            {
                var patch = p;
                card.AddArrangedSubview(SwitchRow("Patch: " + patch.Name, null, patch.On, $"patch-switch-{mi}-{pj++}",
                    on => { GameMods.SetPatch(documents, titleId, mod.Name, patch.Name, on); log?.Invoke($"mods: {mod.Name} patch {patch.Name} {(on ? "on" : "off")}"); }, nested: true));
            }
            int cj = 0;
            foreach (var c in mod.Cheats)
            {
                var cheat = c;
                card.AddArrangedSubview(SwitchRow("Cheat: " + cheat.Name, null, cheat.On, $"cheat-switch-{mi}-{cj++}",
                    on => { GameMods.SetCheat(documents, titleId, mod.Name, cheat.Name, on); log?.Invoke($"mods: {mod.Name} cheat {cheat.Name} {(on ? "on" : "off")}"); }, nested: true));
            }
            var del = UIKit.UIButton.FromType(UIKit.UIButtonType.System);
            var dc = UIKit.UIButtonConfiguration.PlainButtonConfiguration;
            dc.Title = "Delete"; dc.Image = UIKit.UIImage.GetSystemImage("trash"); dc.ImagePadding = 6; dc.BaseForegroundColor = UIKit.UIColor.SystemRed;
            del.Configuration = dc; del.AccessibilityIdentifier = "mod-delete-" + mi; del.HorizontalAlignment = UIKit.UIControlContentHorizontalAlignment.Leading;
            del.TouchUpInside += (_, __) => ConfirmDelete(mod.Name);
            card.AddArrangedSubview(del);
            card.BackgroundColor = UIKit.UIColor.SecondarySystemBackground;
            card.LayoutMargins = new UIKit.UIEdgeInsets(12, 12, 8, 12);
            card.LayoutMarginsRelativeArrangement = true;
            card.Layer.CornerRadius = 12;
            rows.AddArrangedSubview(card);
        }
    }

    private void Say(string title, string message)
    {
        var a = UIKit.UIAlertController.Create(title, message, UIKit.UIAlertControllerStyle.Alert);
        a.AddAction(UIKit.UIAlertAction.Create("OK", UIKit.UIAlertActionStyle.Cancel, null));
        PresentWhenFree(a);
    }

    private void ConfirmDelete(string mod)
    {
        var a = UIKit.UIAlertController.Create("Delete " + mod + "?", $"Its files are removed from this {GameAppUi.DeviceName}. The zip you added it from is not touched.", UIKit.UIAlertControllerStyle.Alert);
        a.AddAction(UIKit.UIAlertAction.Create("Cancel", UIKit.UIAlertActionStyle.Cancel, null));
        a.AddAction(UIKit.UIAlertAction.Create("Delete", UIKit.UIAlertActionStyle.Destructive, _ =>
        {
            try { GameMods.Delete(documents, titleId, mod); log?.Invoke($"mods: {mod} deleted"); } catch (Exception e) { Say("Not deleted", e.Message); }
            Reload();
        }));
        PresentViewController(a, true, null);
    }

    /// Presented once the document picker has gone: an alert presented while it is still going away is dropped by UIKit without a word
    /// (docs/DECISIONS.md trap 56).
    private void PresentWhenFree(UIKit.UIViewController vc, int tries = 20)
    {
        if (PresentedViewController == null) { PresentViewController(vc, true, null); return; }
        if (tries > 0) { Foundation.NSTimer.CreateScheduledTimer(0.15, _ => PresentWhenFree(vc, tries - 1)); return; }
        PresentedViewController.PresentViewController(vc, true, null);
    }

    private void ChooseSource(UIKit.UIView source)
    {
        var sheet = UIKit.UIAlertController.Create("Add mods", "A .zip or a folder holding atmosphere/contents/<title id>/..., a mods folder, or a .pchtxt / .ips patch.", UIKit.UIAlertControllerStyle.ActionSheet);
        sheet.AddAction(UIKit.UIAlertAction.Create("A .zip archive…", UIKit.UIAlertActionStyle.Default, _ => PickZip()));
        sheet.AddAction(UIKit.UIAlertAction.Create("A folder…", UIKit.UIAlertActionStyle.Default, _ => PickFolder()));
        sheet.AddAction(UIKit.UIAlertAction.Create("Cancel", UIKit.UIAlertActionStyle.Cancel, null));
        Theme.Anchor(sheet, source, View);
        PresentViewController(sheet, true, null);
    }

    // Import mode (a copy into tmp/), as the saves' import: Open mode's taps did nothing in a FlareStore install (trap 56).
    private void PickZip()
    {
        var picker = new UIKit.UIDocumentPickerViewController(new[] { "public.zip-archive", "public.data", "public.item" }, UIKit.UIDocumentPickerMode.Import);
        picker.Delegate = new PickerDelegate(url => BeginInvokeOnMainThread(() =>
        {
            var name = url?.LastPathComponent ?? "";
            if (!name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)) { Say("Not a zip", $"{name} is not a .zip. Pick a mod archive, or use \"A folder…\"."); return; }
            var local = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "mod-import-" + Guid.NewGuid().ToString("N") + ".zip");
            try { File.Copy(url.Path, local, true); try { File.Delete(url.Path); } catch { } }
            catch (Exception e) { Say("Not read", e.Message); return; }
            try { Imported(GameMods.ImportPath(documents, titleId, local), name); }
            catch (Exception e) { log?.Invoke($"mods: {name} not added: {e.Message}"); Say("Not added", e.Message); }
            finally { try { File.Delete(local); } catch { } }
        }));
        PresentViewController(picker, true, null);
    }

    private void PickFolder()
    {
        var picker = new UIKit.UIDocumentPickerViewController(new[] { "public.folder" }, UIKit.UIDocumentPickerMode.Open);
        picker.Delegate = new PickerDelegate(url => BeginInvokeOnMainThread(() =>
        {
            if (url == null) return;
            bool scoped = url.StartAccessingSecurityScopedResource();
            try { Imported(GameMods.ImportPath(documents, titleId, url.Path), url.LastPathComponent); }
            catch (Exception e) { log?.Invoke($"mods: {url.LastPathComponent} not added: {e.Message}"); Say("Not added", e.Message); }
            finally { if (scoped) url.StopAccessingSecurityScopedResource(); }
        }));
        PresentViewController(picker, true, null);
    }

    private void Imported(GameMods.ImportResult r, string source)
    {
        log?.Invoke($"mods: {source}: {r.Kept} file(s) kept as {string.Join(", ", r.Mods)}" + (r.Notes.Count > 0 ? " (" + string.Join("; ", r.Notes) + ")" : ""));
        Reload();
        if (r.Notes.Count > 0) Say("Added", $"{string.Join(", ", r.Mods)}: {r.Kept} file(s) kept.\n" + string.Join("\n", r.Notes.Select(n => "• " + n)));
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
