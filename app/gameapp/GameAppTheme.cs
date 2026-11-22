// gameapp/GameAppTheme.cs: the app's look ("Ambient", the design picked on 2026-11-18): a deep night-blue ground lit by
// the colour of the selected game's icon, frosted cards, white pills; the library's covers in a carousel, the storage
// bar of design A on top, the build screen with a ring and a live log (design C, GameAppLibrary.cs). Views only: what they show is decided in
// GameAppUi.cs / GameAppCloud.cs (tested on a Mac).

using System;
using System.Collections.Generic;
using System.Linq;
using CoreAnimation;
using CoreGraphics;
using Foundation;
using UIKit;

internal static class Theme
{
    public static UIColor Hex(int rgb, float alpha = 1f) =>
        UIColor.FromRGBA((rgb >> 16 & 0xFF) / 255f, (rgb >> 8 & 0xFF) / 255f, (rgb & 0xFF) / 255f, alpha);

    public static readonly UIColor Ground = Hex(0x0B0E1A);
    public static readonly UIColor Panel = Hex(0x0F1324, 0.82f);
    public static readonly UIColor Glass = UIColor.FromWhiteAlpha(1f, 0.09f);
    public static readonly UIColor GlassLine = UIColor.FromWhiteAlpha(1f, 0.14f);
    public static readonly UIColor Text = Hex(0xEEF1FF);
    public static readonly UIColor Muted = Hex(0xA3ABC7);
    public static readonly UIColor Dim = Hex(0x8E96B5);
    public static readonly UIColor Ok = Hex(0x7CF0B0);
    public static readonly UIColor Warn = Hex(0xFFC870);
    public static readonly UIColor Error = Hex(0xFF8A8A);
    public static readonly UIColor DefaultTint = Hex(0x6B4BD8);

    public static UIFont Font(nfloat size, UIFontWeight weight) => UIFont.SystemFontOfSize(size, weight);

    public static UIFont Rounded(nfloat size, UIFontWeight weight)
    {
        var f = UIFont.SystemFontOfSize(size, weight);
        var d = f.FontDescriptor.CreateWithDesign(UIFontDescriptorSystemDesign.Rounded);
        return d != null ? UIFont.FromDescriptor(d, size) : f;
    }

    public static UIFont Mono(nfloat size, UIFontWeight weight = default) =>
        UIFont.FromName(weight == UIFontWeight.Bold || weight == UIFontWeight.Semibold ? "Menlo-Bold" : "Menlo-Regular", size) ?? UIFont.SystemFontOfSize(size, UIFontWeight.Regular);

    public static UILabel Label(string text, UIFont font, UIColor color, int lines = 1)
    {
        return new UILabel { Text = text, Font = font, TextColor = color, Lines = lines, AdjustsFontForContentSizeCategory = false };
    }

    /// A frosted card: the ground shows through, a hairline edge.
    public static UIView Card(nfloat radius, UIColor fill = null)
    {
        var v = new UIView { BackgroundColor = fill ?? Glass };
        v.Layer.CornerRadius = radius;
        v.Layer.BorderWidth = 1;
        v.Layer.BorderColor = GlassLine.CGColor;
        return v;
    }

    /// Pins `content` inside `card` with the inset.
    public static UIView Wrap(UIView content, nfloat radius, nfloat inset, UIColor fill = null)
    {
        var card = Card(radius, fill);
        content.TranslatesAutoresizingMaskIntoConstraints = false;
        card.AddSubview(content);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            content.TopAnchor.ConstraintEqualTo(card.TopAnchor, inset),
            content.LeadingAnchor.ConstraintEqualTo(card.LeadingAnchor, inset),
            content.TrailingAnchor.ConstraintEqualTo(card.TrailingAnchor, -inset),
            content.BottomAnchor.ConstraintEqualTo(card.BottomAnchor, -inset),
        });
        return card;
    }

    /// The white pill (the screen's one main action) or a glass one.
    public static UIButton Pill(string title, string symbol, bool primary, nfloat height = default)
    {
        var b = new UIButton(UIButtonType.System);
        var c = primary ? UIButtonConfiguration.FilledButtonConfiguration : UIButtonConfiguration.GrayButtonConfiguration;
        c.AttributedTitle = new NSAttributedString(title, new UIStringAttributes { Font = Font(16, UIFontWeight.Semibold) });
        if (symbol != null) { c.Image = UIImage.GetSystemImage(symbol); c.ImagePadding = 8; }
        c.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
        c.BaseBackgroundColor = primary ? UIColor.White : UIColor.FromWhiteAlpha(1f, 0.12f);
        c.BaseForegroundColor = primary ? Ground : Text;
        b.Configuration = c;
        b.HeightAnchor.ConstraintEqualTo(height == default ? 56 : height).Active = true;
        b.PointerInteractionEnabled = true;
        return b;
    }

    /// A round glass button with a symbol (44 pt: the smallest a finger hits).
    public static UIButton Round(string symbol, string label, nfloat size = default)
    {
        var s = size == default ? 44 : size;
        var b = new UIButton(UIButtonType.System);
        var c = UIButtonConfiguration.PlainButtonConfiguration;
        c.Image = UIImage.GetSystemImage(symbol);
        c.PreferredSymbolConfigurationForImage = UIImageSymbolConfiguration.Create(UIFont.SystemFontOfSize(17, UIFontWeight.Semibold));
        c.BaseForegroundColor = Text;
        b.Configuration = c;
        b.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.10f);
        b.Layer.CornerRadius = s / 2;
        b.Layer.BorderWidth = 1;
        b.Layer.BorderColor = GlassLine.CGColor;
        b.AccessibilityLabel = label;
        b.PointerInteractionEnabled = true;
        b.TranslatesAutoresizingMaskIntoConstraints = false;
        b.WidthAnchor.ConstraintEqualTo(s).Active = true;
        b.HeightAnchor.ConstraintEqualTo(s).Active = true;
        return b;
    }

    /// Pins `column` in a vertically scrolling `scroll`: as wide as the scroll (less `side` each way) up to `max`, centred. A phone
    /// never reaches the maximum, so there it is the old edge-to-edge column; on an iPad a page does not stretch into a wide line.
    /// Returns the maximum-width constraint, for a screen that changes it.
    public static NSLayoutConstraint PinColumn(UIScrollView scroll, UIView column, nfloat top, nfloat bottom, nfloat side = default, nfloat max = default)
    {
        if (max == default) max = 700;
        var limit = column.WidthAnchor.ConstraintLessThanOrEqualTo(max);
        var width = column.WidthAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.WidthAnchor, 1, -2 * side);
        width.Priority = 999;   // the maximum wins when the scroll is wider; 999 and not 750, which the labels' own 750 compression resistance tied with
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            scroll.ContentLayoutGuide.WidthAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.WidthAnchor),
            column.TopAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.TopAnchor, top),
            column.BottomAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.BottomAnchor, -bottom),
            column.CenterXAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.CenterXAnchor).WithPriority(750),
            column.LeadingAnchor.ConstraintGreaterThanOrEqualTo(scroll.ContentLayoutGuide.LeadingAnchor),   // never off the left edge, whatever the rows need
            limit,
            width,
        });
        return limit;
    }

    private static NSLayoutConstraint WithPriority(this NSLayoutConstraint c, float priority) { c.Priority = priority; return c; }

    /// The trackpad's hover effect on every button below `root` (an iPad with a pointer; nothing happens on a phone). Called again
    /// after a list is rebuilt, because its buttons are new.
    public static void Hover(UIView root)
    {
        if (root is UIButton b) b.PointerInteractionEnabled = true;
        foreach (var s in root.Subviews) Hover(s);
    }

    /// An action sheet or share sheet on an iPad is a popover and needs a place to point at, or UIKit raises an exception. `source` is
    /// the control that was tapped; without one the popover opens in the middle of `host`, with no arrow.
    public static void Anchor(UIViewController sheet, UIView source, UIView host)
    {
        var p = sheet?.PopoverPresentationController;
        if (p == null) return;
        var view = source?.Window != null ? source : host;
        if (view == null) return;
        p.SourceView = view;
        if (view == source) p.SourceRect = view.Bounds;
        else
        {
            p.SourceRect = new CGRect(view.Bounds.GetMidX(), view.Bounds.GetMidY(), 1, 1);
            p.PermittedArrowDirections = 0;
        }
    }

    /// How a settings, mods or saves sheet opens. A phone: the page sheet that rests at half height or opens fully. An iPad: a form sheet
    /// of a readable size in the middle of the screen (a page sheet there is nearly the whole window, with rows 900 pt wide); in a
    /// narrow window UIKit turns a form sheet into the full-width page by itself.
    public static void StyleSheet(UIViewController sheet)
    {
        if (IsPad)
        {
            sheet.ModalPresentationStyle = UIModalPresentationStyle.FormSheet;
            sheet.PreferredContentSize = new CGSize(640, 900);
            return;
        }
        sheet.ModalPresentationStyle = UIModalPresentationStyle.PageSheet;
        var presentation = sheet.SheetPresentationController;
        if (presentation != null)
            presentation.Detents = new[] { UISheetPresentationControllerDetent.CreateMediumDetent(), UISheetPresentationControllerDetent.CreateLargeDetent() };
    }

    /// iPhone or iPad, for the words that name the device ("this iPad").
    public static bool IsPad => UIDevice.CurrentDevice.UserInterfaceIdiom == UIUserInterfaceIdiom.Pad;

    /// A symbol at a fixed size (an image view left to itself stretches to fill its stack).
    public static UIImageView Symbol(string name, nfloat size, UIColor tint = null)
    {
        var v = new UIImageView(UIImage.GetSystemImage(name)) { TintColor = tint ?? Text, ContentMode = UIViewContentMode.ScaleAspectFit };
        v.TranslatesAutoresizingMaskIntoConstraints = false;
        v.WidthAnchor.ConstraintEqualTo(size).Active = true;
        v.HeightAnchor.ConstraintEqualTo(size).Active = true;
        v.SetContentHuggingPriority(1000, UILayoutConstraintAxis.Horizontal);
        return v;
    }

    public static UIStackView Row(nfloat spacing, params UIView[] views) =>
        new UIStackView(views) { Axis = UILayoutConstraintAxis.Horizontal, Alignment = UIStackViewAlignment.Center, Spacing = spacing };

    public static UIStackView Column(nfloat spacing, params UIView[] views) =>
        new UIStackView(views) { Axis = UILayoutConstraintAxis.Vertical, Spacing = spacing };

    /// The icon's own colour: its average, made a little brighter and more saturated so it lights the ground.
    public static UIColor AverageColor(UIImage image)
    {
        try
        {
            if (image == null) return null;
            if (averages.TryGetValue(image, out var known)) return known;
            var cg = image.CGImage;
            if (cg == null) return null;
            var data = new byte[4];
            using var space = CGColorSpace.CreateDeviceRGB();
            using var ctx = new CGBitmapContext(data, 1, 1, 8, 4, space, CGImageAlphaInfo.PremultipliedLast);
            ctx.InterpolationQuality = CGInterpolationQuality.Medium;
            ctx.DrawImage(new CGRect(0, 0, 1, 1), cg);
            var c = UIColor.FromRGB(data[0], data[1], data[2]);
            c.GetHSBA(out var h, out var s, out var b, out _);
            var tint = UIColor.FromHSBA(h, (nfloat) Math.Min(1.0, s * 1.25 + 0.1), (nfloat) Math.Max(0.55, Math.Min(0.95, b * 1.1)), 1);
            averages.Add(image, tint);
            return tint;
        }
        catch (Exception) { return null; }
    }

    // One average per decoded image: the library's cover cache hands out the same UIImage until its file changes
    // (main thread only).
    private static readonly System.Runtime.CompilerServices.ConditionalWeakTable<UIImage, UIColor> averages = new();

    /// A game with no icon yet: a colour of its own (from its name) and its initials.
    public static UIColor ColorFor(string name)
    {
        unchecked
        {
            int h = 17;
            foreach (var ch in name ?? "") h = h * 31 + ch;
            return Hex(Palette[(h & 0x7FFFFFFF) % Palette.Length]);
        }
    }

    private static readonly int[] Palette = { 0x6B4BD8, 0xD8322F, 0xF2C230, 0x2FB6A0, 0xFF6FAE, 0x3D8BFF, 0xFF8A3D, 0x8BC34A };
    private static readonly HashSet<string> SmallWords = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "and", "the", "of", "a", "an", "&", "de", "la", "le", "les", "et", "du" };

    public static string Initials(string displayName)
    {
        var all = (displayName ?? "?").Split(new[] { ' ', ':', '-', '_' }, StringSplitOptions.RemoveEmptyEntries)
                                       .Where(w => char.IsLetterOrDigit(w[0])).ToList();
        var words = all.Where((w, i) => i == 0 || !SmallWords.Contains(w)).ToList();
        if (words.Count == 0) return "?";
        return (words.Count == 1 ? words[0].Substring(0, Math.Min(2, words[0].Length)) : "" + words[0][0] + words[1][0]).ToUpperInvariant();
    }
}

/// The ground: night blue, lit from the top by the selected game's colour (a soft radial light that follows it).
internal sealed class AmbientView : UIView
{
    private readonly CAGradientLayer glow = new CAGradientLayer();

    public AmbientView()
    {
        BackgroundColor = Theme.Ground;
        UserInteractionEnabled = false;
        glow.LayerType = CAGradientLayerType.Radial;
        glow.StartPoint = new CGPoint(0.5, 0.5);
        glow.EndPoint = new CGPoint(1, 1);
        Layer.AddSublayer(glow);
        SetTint(Theme.DefaultTint, false);
    }

    public void SetTint(UIColor tint, bool animated)
    {
        var c = (tint ?? Theme.DefaultTint);
        CATransaction.Begin();
        CATransaction.AnimationDuration = animated ? 0.6 : 0;
        CATransaction.DisableActions = !animated;
        glow.Colors = new[] { c.ColorWithAlpha(0.62f).CGColor, c.ColorWithAlpha(0.22f).CGColor, c.ColorWithAlpha(0f).CGColor };
        glow.Locations = new NSNumber[] { 0, 0.45, 1 };
        CATransaction.Commit();
    }

    public override void LayoutSubviews()
    {
        base.LayoutSubviews();
        var w = Bounds.Width;
        glow.Frame = new CGRect(-w * 0.45, -w * 0.75, w * 1.9, w * 1.75);
    }
}

/// Design A's bar: games, builds, shaders (and exported apps), then the free space, with a legend.
internal sealed class StorageBarView : UIView
{
    private static readonly UIColor[] Colors = { Theme.Hex(0x9DB7FF), Theme.Hex(0xC8A6FF), Theme.Hex(0xFF9CC9), Theme.Hex(0x7CF0B0), UIColor.FromWhiteAlpha(1f, 0.16f) };
    private readonly UIView track = new UIView();
    private readonly List<UIView> parts = new List<UIView>();
    private readonly UILabel legend = new UILabel { Lines = 0 };
    private List<GameAppUi.StorageSegment> segments = new List<GameAppUi.StorageSegment>();

    public StorageBarView()
    {
        track.Layer.CornerRadius = 4;
        track.ClipsToBounds = true;
        track.TranslatesAutoresizingMaskIntoConstraints = false;
        legend.TranslatesAutoresizingMaskIntoConstraints = false;
        AddSubview(track);
        AddSubview(legend);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            track.TopAnchor.ConstraintEqualTo(TopAnchor), track.LeadingAnchor.ConstraintEqualTo(LeadingAnchor),
            track.TrailingAnchor.ConstraintEqualTo(TrailingAnchor), track.HeightAnchor.ConstraintEqualTo(8),
            legend.TopAnchor.ConstraintEqualTo(track.BottomAnchor, 8), legend.LeadingAnchor.ConstraintEqualTo(LeadingAnchor),
            legend.TrailingAnchor.ConstraintEqualTo(TrailingAnchor), legend.BottomAnchor.ConstraintEqualTo(BottomAnchor),
        });
        IsAccessibilityElement = true;
    }

    public void Set(List<GameAppUi.StorageSegment> list)
    {
        segments = list ?? new List<GameAppUi.StorageSegment>();
        foreach (var p in parts) p.RemoveFromSuperview();
        parts.Clear();
        var text = new NSMutableAttributedString();
        for (int i = 0; i < segments.Count; i++)
        {
            var color = segments[i].Label == "Free" ? Colors[4] : Colors[Math.Min(i, 3)];
            var part = new UIView { BackgroundColor = color };
            track.AddSubview(part);
            parts.Add(part);
            if (segments[i].Label == "Free")
            {
                text.Append(new NSAttributedString("   " + GameAppUi.SizeText(segments[i].Bytes) + " free", new UIStringAttributes { Font = Theme.Font(12, UIFontWeight.Medium), ForegroundColor = Theme.Muted }));
                continue;
            }
            if (segments[i].Bytes <= 0) continue;
            text.Append(new NSAttributedString((text.Length > 0 ? "   " : "") + "● ", new UIStringAttributes { Font = Theme.Font(12, UIFontWeight.Bold), ForegroundColor = color }));
            text.Append(new NSAttributedString(segments[i].Label + " " + GameAppUi.SizeText(segments[i].Bytes), new UIStringAttributes { Font = Theme.Font(12, UIFontWeight.Medium), ForegroundColor = Theme.Muted }));
        }
        legend.AttributedText = text;
        AccessibilityLabel = "Storage: " + string.Join(", ", segments.Select(s => s.Label + " " + GameAppUi.SizeText(s.Bytes)));
        SetNeedsLayout();
    }

    public override void LayoutSubviews()
    {
        base.LayoutSubviews();
        double total = Math.Max(1, segments.Sum(s => (double) s.Bytes));
        nfloat x = 0, w = track.Bounds.Width;
        for (int i = 0; i < parts.Count && i < segments.Count; i++)
        {
            var width = (nfloat) (segments[i].Bytes / total) * w;
            if (segments[i].Bytes > 0 && width < 3) width = 3;
            parts[i].Frame = new CGRect(x, 0, Math.Max(0, Math.Min(width, w - x)), 8);
            x += width + (width > 0 ? 2 : 0);
        }
    }
}

/// One cover: the game's icon (or its colour and initials), rounded; in the cloud it is dimmed with a cloud mark; while
/// downloading or building, a ring shows how far.
internal sealed class CoverView : UIView
{
    private readonly UIImageView image = new UIImageView { ContentMode = UIViewContentMode.ScaleAspectFill, ClipsToBounds = true };
    private readonly UILabel initials = new UILabel { TextAlignment = UITextAlignment.Center, TextColor = UIColor.White };
    private readonly UIImageView cloud = new UIImageView(UIImage.GetSystemImage("icloud.and.arrow.down.fill")) { TintColor = UIColor.White, Hidden = true };
    private readonly CAShapeLayer ringBack = new CAShapeLayer(), ring = new CAShapeLayer();
    private readonly UILabel percent = new UILabel { TextAlignment = UITextAlignment.Center, TextColor = UIColor.White, Hidden = true };

    public CoverView()
    {
        ClipsToBounds = false;
        AddSubview(image);
        AddSubview(initials);
        AddSubview(cloud);
        foreach (var l in new[] { ringBack, ring })
        {
            l.FillColor = UIColor.Clear.CGColor;
            l.LineCap = CAShapeLayer.CapRound;
            l.Hidden = true;
            Layer.AddSublayer(l);
        }
        ringBack.StrokeColor = UIColor.FromWhiteAlpha(1f, 0.25f).CGColor;
        ring.StrokeColor = UIColor.White.CGColor;
        AddSubview(percent);
        IsAccessibilityElement = false;
    }

    public void Set(UIImage icon, string name, bool inCloud, double? fraction)
    {
        image.Image = icon;
        image.BackgroundColor = icon == null ? Theme.ColorFor(name) : UIColor.Clear;
        initials.Text = icon == null ? Theme.Initials(name) : "";
        image.Alpha = inCloud ? 0.45f : 1f;
        initials.Alpha = inCloud ? 0.6f : 1f;
        cloud.Hidden = !inCloud || fraction.HasValue;
        ring.Hidden = ringBack.Hidden = percent.Hidden = !fraction.HasValue;
        if (fraction.HasValue)
        {
            ring.StrokeEnd = (nfloat) Math.Max(0.02, Math.Min(1, fraction.Value));
            percent.Text = $"{Math.Round(fraction.Value * 100):0}%";
        }
        SetNeedsLayout();
    }

    public override void LayoutSubviews()
    {
        base.LayoutSubviews();
        var b = Bounds;
        image.Frame = b;
        image.Layer.CornerRadius = b.Width * 0.19f;
        initials.Frame = b;
        initials.Font = Theme.Rounded(b.Width * 0.27f, UIFontWeight.Bold);
        nfloat cs = (nfloat) Math.Max(18.0, (double) (b.Width * 0.16f));
        cloud.Frame = new CGRect(b.Width - cs - b.Width * 0.08f, b.Height - cs * 0.85f - b.Width * 0.08f, cs, cs * 0.85f);
        var r = b.Width * 0.32f;
        var path = UIBezierPath.FromArc(new CGPoint(b.GetMidX(), b.GetMidY()), r, (nfloat) (-Math.PI / 2), (nfloat) (Math.PI * 1.5), true).CGPath;
        foreach (var l in new[] { ringBack, ring }) { l.Path = path; l.LineWidth = (nfloat) Math.Max(4.0, (double) (b.Width * 0.045f)); l.Frame = b; }
        nfloat dimmer = b.Width * 0.64f;
        percent.Frame = new CGRect(b.GetMidX() - dimmer / 2, b.GetMidY() - 14, dimmer, 28);
        percent.Font = Theme.Rounded((nfloat) Math.Max(13.0, (double) (b.Width * 0.11f)), UIFontWeight.Bold);
    }
}

/// The library's carousel: the covers side by side, the one in front large, its neighbours peeking and smaller. It
/// settles on a cover (never between two) and says which one is in front.
internal sealed class CoverCarousel : UIScrollView
{
    /// The front cover's side: 220 on a phone, larger in the iPad's detail pane (the library sets it from the pane's width).
    public nfloat Item = 220;
    public const float Gap = 18;
    private readonly List<CoverView> covers = new List<CoverView>();
    private int current = -1;
    private nfloat laidOutWidth;
    public event Action<int> Settled;
    public int Count => covers.Count;

    public CoverCarousel()
    {
        ShowsHorizontalScrollIndicator = false;
        ShowsVerticalScrollIndicator = false;
        AlwaysBounceVertical = false;
        ContentInsetAdjustmentBehavior = UIScrollViewContentInsetAdjustmentBehavior.Never;
        DirectionalLockEnabled = true;
        DecelerationRate = UIScrollView.DecelerationRateFast;
        ClipsToBounds = false;
        Delegate = new Snap(this);
    }

    public void SetItems(IReadOnlyList<(UIImage Icon, string Name, bool Cloud, double? Fraction)> items)
    {
        while (covers.Count > items.Count) { covers[^1].RemoveFromSuperview(); covers.RemoveAt(covers.Count - 1); }
        while (covers.Count < items.Count)
        {
            var c = new CoverView();
            int index = covers.Count;
            c.AddGestureRecognizer(new UITapGestureRecognizer(() => Select(index, true)));
            AddSubview(c);
            covers.Add(c);
        }
        for (int i = 0; i < items.Count; i++) covers[i].Set(items[i].Icon, items[i].Name, items[i].Cloud, items[i].Fraction);
        SetNeedsLayout();
    }

    private nfloat Side => (Bounds.Width - Item) / 2;

    /// A new cover size (the pane changed): the covers are laid out again and the front one stays in front.
    public void SetItemSize(nfloat item)
    {
        if (item == Item) return;
        Item = item;
        SetNeedsLayout();
        LayoutIfNeeded();
        if (current >= 0) SetContentOffset(new CGPoint(current * (Item + Gap) - Side, 0), false);
    }

    public void Select(int index, bool animated)
    {
        if (index < 0 || index >= covers.Count) return;
        current = index;
        LayoutIfNeeded();
        SetContentOffset(new CGPoint(index * (Item + Gap) - Side, 0), animated);
        if (!animated) Settled?.Invoke(index);
    }

    public override void LayoutSubviews()
    {
        base.LayoutSubviews();
        var h = Bounds.Height;
        ContentInset = new UIEdgeInsets(0, Side, 0, Side);
        ContentSize = new CGSize(Math.Max(0, covers.Count * (Item + Gap) - Gap), h);
        // The pane was resized (rotation, Split View, the library's two panes): the front cover stays in the middle.
        if (Bounds.Width != laidOutWidth)
        {
            laidOutWidth = Bounds.Width;
            if (current >= 0 && !Dragging && !Decelerating) ContentOffset = new CGPoint(current * (Item + Gap) - Side, 0);
        }
        for (int i = 0; i < covers.Count; i++)
        {
            covers[i].Transform = CGAffineTransform.MakeIdentity();
            covers[i].Frame = new CGRect(i * (Item + Gap), (h - Item) / 2, Item, Item);
        }
        Shape();
    }

    /// Covers away from the centre shrink and fade: the one in front reads as the selection.
    private void Shape()
    {
        var centre = ContentOffset.X + Bounds.Width / 2;
        for (int i = 0; i < covers.Count; i++)
        {
            var d = Math.Min(1.0, Math.Abs(i * (Item + Gap) + Item / 2 - centre) / (Item + Gap));
            var s = (nfloat) (1 - 0.24 * d);
            covers[i].Transform = CGAffineTransform.MakeScale(s, s);
            covers[i].Layer.ShadowColor = UIColor.Black.CGColor;
            covers[i].Layer.ShadowOpacity = (float) (0.45 * (1 - d));
            covers[i].Layer.ShadowRadius = 26;
            covers[i].Layer.ShadowOffset = new CGSize(0, 18);
        }
    }

    private int Nearest(nfloat x) => (int) Math.Max(0, Math.Min(covers.Count - 1, Math.Round((x + Side) / (Item + Gap))));

    private sealed class Snap : UIScrollViewDelegate
    {
        private readonly CoverCarousel owner;
        public Snap(CoverCarousel owner) => this.owner = owner;
        public override void Scrolled(UIScrollView scrollView) => owner.Shape();
        public override void WillEndDragging(UIScrollView scrollView, CGPoint velocity, ref CGPoint targetContentOffset)
        {
            int i = owner.Nearest(targetContentOffset.X);
            targetContentOffset = new CGPoint(i * (owner.Item + Gap) - owner.Side, targetContentOffset.Y);
        }
        public override void DecelerationEnded(UIScrollView scrollView) => Done();
        public override void DraggingEnded(UIScrollView scrollView, bool willDecelerate) { if (!willDecelerate) Done(); }
        public override void ScrollAnimationEnded(UIScrollView scrollView) => Done();
        private void Done()
        {
            int i = owner.Nearest(owner.ContentOffset.X);
            owner.current = i;
            owner.Settled?.Invoke(i);
        }
    }
}

/// A row of the library's list: the icon, the name, a line under it, and a trailing button (Play, Build, Download, a
/// percentage). A game only in the cloud is dimmed with a cloud mark. "…" opens its menu.
internal sealed class GameRowView : UIControl
{
    public readonly UIButton Trailing = new UIButton(UIButtonType.System);
    public readonly UIButton More;

    public GameRowView(UIImage icon, string name, string subtitle, bool inCloud, bool selected, string trailingTitle, string trailingSymbol, bool trailingPrimary, UIMenu menu)
    {
        var cover = new CoverView();
        cover.Set(icon, name, inCloud, null);
        cover.TranslatesAutoresizingMaskIntoConstraints = false;
        cover.WidthAnchor.ConstraintEqualTo(52).Active = true;
        cover.HeightAnchor.ConstraintEqualTo(52).Active = true;
        cover.UserInteractionEnabled = false;
        var title = Theme.Label(name, Theme.Font(16, UIFontWeight.Semibold), inCloud ? Theme.Muted : Theme.Text, 2);
        var sub = Theme.Label(subtitle, Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 2);
        var texts = Theme.Column(2, title, sub);
        texts.UserInteractionEnabled = false;
        texts.SetContentHuggingPriority(1, UILayoutConstraintAxis.Horizontal);
        texts.SetContentCompressionResistancePriority(1, UILayoutConstraintAxis.Horizontal);

        var c = trailingPrimary ? UIButtonConfiguration.FilledButtonConfiguration : UIButtonConfiguration.GrayButtonConfiguration;
        c.CornerStyle = UIButtonConfigurationCornerStyle.Capsule;
        c.BaseBackgroundColor = trailingPrimary ? UIColor.White : UIColor.FromWhiteAlpha(1f, 0.12f);
        c.BaseForegroundColor = trailingPrimary ? Theme.Ground : Theme.Text;
        if (!string.IsNullOrEmpty(trailingTitle)) c.Title = trailingTitle;
        if (trailingSymbol != null) { c.Image = UIImage.GetSystemImage(trailingSymbol); c.ImagePadding = 6; }
        c.ContentInsets = new NSDirectionalEdgeInsets(10, 14, 10, 14);
        Trailing.Configuration = c;
        Trailing.Hidden = trailingTitle == null && trailingSymbol == null;
        Trailing.SetContentHuggingPriority(1000, UILayoutConstraintAxis.Horizontal);
        Trailing.SetContentCompressionResistancePriority(1000, UILayoutConstraintAxis.Horizontal);
        Trailing.HeightAnchor.ConstraintGreaterThanOrEqualTo(44).Active = true;
        var verb = !string.IsNullOrEmpty(trailingTitle) ? trailingTitle : trailingSymbol != null && trailingSymbol.Contains("arrow.down") ? "Download" : "";
        Trailing.AccessibilityLabel = (verb + " " + name).Trim();

        More = new UIButton(UIButtonType.System);
        var mc = UIButtonConfiguration.PlainButtonConfiguration;
        mc.Image = UIImage.GetSystemImage("ellipsis");
        mc.BaseForegroundColor = Theme.Muted;
        More.Configuration = mc;
        More.Menu = menu;
        More.ShowsMenuAsPrimaryAction = true;
        More.Hidden = menu == null;
        More.AccessibilityLabel = "More for " + name;
        More.WidthAnchor.ConstraintEqualTo(32).Active = true;
        More.HeightAnchor.ConstraintEqualTo(44).Active = true;

        var row = Theme.Row(12, cover, texts, Trailing, More);
        row.TranslatesAutoresizingMaskIntoConstraints = false;
        AddSubview(row);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            row.TopAnchor.ConstraintEqualTo(TopAnchor, 8), row.BottomAnchor.ConstraintEqualTo(BottomAnchor, -8),
            row.LeadingAnchor.ConstraintEqualTo(LeadingAnchor, 10), row.TrailingAnchor.ConstraintEqualTo(TrailingAnchor, -4),
        });
        Layer.CornerRadius = 18;
        BackgroundColor = selected ? UIColor.FromWhiteAlpha(1f, 0.08f) : UIColor.Clear;
        AddInteraction(new UIPointerInteraction(new RowPointer()));   // a trackpad lifts the row; Play and "…" keep their own effect
        AccessibilityLabel = name + ", " + subtitle;
    }
}

/// The hover effect of a library row (a UIControl, not a UIButton: it has no pointer switch of its own).
internal sealed class RowPointer : UIPointerInteractionDelegate
{
    public override UIPointerStyle GetStyleForRegion(UIPointerInteraction interaction, UIPointerRegion region)
        => UIPointerStyle.Create(UIPointerHighlightEffect.Create(new UITargetedPreview(interaction.View)), (UIPointerShape) null);
}

/// One row of the log's timeline (design C): the time on the left, a dot on a vertical line (red with a halo for an
/// error, amber for a warning), and on the right the step it opens (small capitals) above the line itself.
internal sealed class TimelineCell : UITableViewCell
{
    public const string Id = "timeline";
    private readonly UILabel time = new UILabel(), header = new UILabel(), text = new UILabel();
    private readonly UIView dot = new UIView(), line = new UIView(), halo = new UIView();

    public TimelineCell(ObjCRuntime.NativeHandle handle) : base(handle) => Build();   // made by the table (RegisterClassForCellReuse)

    private void Build()
    {
        BackgroundColor = UIColor.Clear;
        ContentView.BackgroundColor = UIColor.Clear;
        time.Font = Theme.Mono(11);
        time.TextColor = Theme.Dim;
        time.TextAlignment = UITextAlignment.Right;
        header.Font = Theme.Font(11, UIFontWeight.Semibold);
        header.TextColor = Theme.Muted;
        text.Lines = 0;
        text.Font = Theme.Font(14, UIFontWeight.Regular);
        line.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.12f);
        dot.Layer.CornerRadius = 6;
        halo.Layer.CornerRadius = 10;
        foreach (var v in new UIView[] { time, line, halo, dot, header, text })
        {
            v.TranslatesAutoresizingMaskIntoConstraints = false;
            ContentView.AddSubview(v);
        }
        var c = ContentView;
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            time.LeadingAnchor.ConstraintEqualTo(c.LeadingAnchor, 2), time.WidthAnchor.ConstraintEqualTo(42),
            time.FirstBaselineAnchor.ConstraintEqualTo(text.FirstBaselineAnchor),
            dot.WidthAnchor.ConstraintEqualTo(12), dot.HeightAnchor.ConstraintEqualTo(12),
            dot.CenterXAnchor.ConstraintEqualTo(time.TrailingAnchor, 16), dot.CenterYAnchor.ConstraintEqualTo(text.TopAnchor, 9),
            halo.WidthAnchor.ConstraintEqualTo(20), halo.HeightAnchor.ConstraintEqualTo(20),
            halo.CenterXAnchor.ConstraintEqualTo(dot.CenterXAnchor), halo.CenterYAnchor.ConstraintEqualTo(dot.CenterYAnchor),
            line.WidthAnchor.ConstraintEqualTo(2), line.CenterXAnchor.ConstraintEqualTo(dot.CenterXAnchor),
            line.TopAnchor.ConstraintEqualTo(dot.BottomAnchor, 2), line.BottomAnchor.ConstraintEqualTo(c.BottomAnchor, 6),
            header.TopAnchor.ConstraintEqualTo(c.TopAnchor, 2),
            header.LeadingAnchor.ConstraintEqualTo(dot.CenterXAnchor, 18), header.TrailingAnchor.ConstraintEqualTo(c.TrailingAnchor, -6),
            text.TopAnchor.ConstraintEqualTo(header.BottomAnchor, 2),
            text.LeadingAnchor.ConstraintEqualTo(header.LeadingAnchor), text.TrailingAnchor.ConstraintEqualTo(header.TrailingAnchor),
            text.BottomAnchor.ConstraintEqualTo(c.BottomAnchor, -14),
        });
    }

    public void Set(GameAppUi.TimelineRow row, bool last)
    {
        time.Text = row.Time;
        header.Text = string.IsNullOrEmpty(row.Header) ? "" : row.Header.ToUpperInvariant();
        var (colour, weight, dotColour) = row.Kind switch
        {
            GameAppUi.LogKind.Error => (Theme.Error, UIFontWeight.Semibold, Theme.Hex(0xFF6B6B)),
            GameAppUi.LogKind.Warning => (Theme.Hex(0xFFD58F), UIFontWeight.Regular, Theme.Warn),
            GameAppUi.LogKind.Stage => (Theme.Text, UIFontWeight.Semibold, UIColor.White),
            _ => (Theme.Text, UIFontWeight.Regular, UIColor.FromWhiteAlpha(1f, 0.55f)),
        };
        text.Text = row.Text;
        text.TextColor = colour;
        text.Font = Theme.Font(14, weight);
        dot.BackgroundColor = dotColour;
        halo.BackgroundColor = Theme.Hex(0xFF6B6B, 0.25f);
        halo.Hidden = row.Kind != GameAppUi.LogKind.Error;
        line.Hidden = last;
        IsAccessibilityElement = true;
        AccessibilityLabel = $"{row.Time}, {(string.IsNullOrEmpty(row.Header) ? "" : row.Header + ", ")}{(row.Kind == GameAppUi.LogKind.Error ? "error: " : row.Kind == GameAppUi.LogKind.Warning ? "warning: " : "")}{row.Text}";
    }
}
