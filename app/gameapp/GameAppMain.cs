// gameapp/GameAppMain.cs -- the capstone app: a game in Documents becomes a RUNNING game, in one process.
//
// One launch does the whole thing: the engine's extraction (P.Main, the tool's own Program.cs compiled into
// this app, called directly -- the runtime is started by the iOS SDK's own main.m, which is the whole reason
// this road works where a hand-rolled mono_init_version crashes inside Mono's GC), then the chain -- rewrite,
// shaders, the no-linker assembly, the in-process zsign signature and the dlopen + boot to nnMain -- through
// the P/Invoke into gameapp/gameapp_shim.c, which starts CPython in THIS process (SAME pid).
//
// What this file adds to the apps that already exist here (app-net/BuilderApp.cs, oneproc/OneProcMain.cs):
//
//   * the memory lines. The rewrite peaks near 3 GiB in one process, and jetsam on an iPhone is the one
//     number that decides whether any of this works on hardware. Resident/peak/footprint come from the
//     kernel (gameapp_memline, task_info), and free space from statfs, before and after every heavy step.
//   * the .ipa is the device build of this same file; gameapp/verify_ipa.sh checks the DEVICE bundle structurally
//     (platform 2, its frameworks, its Python.framework) and the strings it must carry.
//
// The Files-app folder (UIFileSharingEnabled + LSSupportsOpeningDocumentsInPlace in the Info.plist fragment), the
// document picker and the log timeline are part of the app. Documents/gameapp.args is a ONE-SHOT request: if it is
// present (the harness left it, or a previous tap wrote it) and no run was interrupted, the run starts once by itself; if a previous run was
// interrupted (gameapp.running is still there) the app reports it and does NOT start again. The stored request is
// a set of NAMES, re-anchored under the CURRENT Documents at every launch -- never a remembered absolute path,
// which an app reinstall turns into a dead container reference.
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

using CoreAnimation;
using CoreGraphics;
using Foundation;
using Metal;
using ObjCRuntime;
using UIKit;

public static class GameAppMain
{
    // gameapp/gameapp_shim.c, linked into this executable (NativeReference Kind=Static).
    [DllImport("__Internal")] internal static extern int gameapp_run_chain();
    [DllImport("__Internal")] private static extern int gameapp_memline(byte[] buffer, int length);
    [DllImport("__Internal")] private static extern long gameapp_available();
    [DllImport("__Internal")] private static extern int gameapp_extended_va();
    [DllImport("__Internal")] private static extern int gameapp_increased_memory();
    [DllImport("__Internal")] private static extern int gameapp_get_task_allow();
    [DllImport("__Internal")] private static extern void gameapp_address_space(out long freeTotal, out long largest);
    // The signing identity, read and written by the shim itself: one implementation of the Keychain item, so the
    // status the UI prints is the same one device_identity_sign() will read.
    [DllImport("__Internal")] private static extern int gameapp_identity_status(out long p12Len);
    [DllImport("__Internal")] private static extern int gameapp_identity_store_bytes(byte[] p12, long length, string passphrase);
    [DllImport("__Internal")] private static extern int gameapp_identity_where();
    // The game's window. The CAMetalLayer of the game view goes to the shim, which hands it to the host the moment the
    // framework is dlopen'd; the engine presents into it (front_metal_layer). Quit ends the guest the way the time
    // limit does, with its own line in the chain log.
    [DllImport("__Internal")] internal static extern void gameapp_set_game_layer(IntPtr layer);
    [DllImport("__Internal")] internal static extern void gameapp_quit_game();
    // NSZ -> NSP (gameapp/nsz_decompress.c through gameapp_shim.c, which logs to the chain log). 0 = done.
    [DllImport("__Internal")] private static extern int gameapp_nsz(string src, string dst, byte[] err, int errlen);
    // The engine's touch gamepad, from the host the boot dlopen'd (gameapp_shim.c, load/front_pad.m). why: 0 made,
    // 1 the host is not loaded yet, 2 this host has no gamepad. Main thread.
    [DllImport("__Internal")] internal static extern IntPtr gameapp_pad_create(double width, double height, out int why);
    [DllImport("__Internal")] internal static extern int gameapp_pad_layout(IntPtr pad, double width, double height,
        double top, double left, double bottom, double right, [Out] double[] picture);

    public static void Main(string[] args)
    {
        RegisterShaderTranslator();
        // The library's icons are read through the extractor's private functions: say once whether trimming kept them.
        try { GameIcons.Read(null, null); Console.Error.WriteLine("[gameapp] icon reader: the extractor's OpenNsp / Pick / Icon are here"); }
        catch (Exception error) { Console.Error.WriteLine($"[gameapp] icon reader: [RED] {error.Message}"); }
        UIApplication.Main(args, null, typeof(GameAppAppDelegate));
    }

    // The engine's shader translator (dmsh_xlate over Ryujinx.Graphics.Shader, gameapp/xlate/) runs in this runtime: the
    // chain's Python reaches it through the shim (_dmgameapp.xlate), which calls this entry with the tool's own command line.
    [DllImport("__Internal")] private static extern unsafe void gameapp_set_xlate(delegate* unmanaged<int, IntPtr, IntPtr, IntPtr, int> fn);

    private static unsafe void RegisterShaderTranslator()
    {
        try { gameapp_set_xlate(&XlateEntry); }
        catch (Exception error) { Console.Error.WriteLine($"[gameapp] shader translator not registered: {error.Message}"); }
    }

    private static long xlateCount;

    [System.Runtime.InteropServices.UnmanagedCallersOnly]
    private static int XlateEntry(int argc, IntPtr argv, IntPtr stdoutOut, IntPtr stderrOut)
    {
        string so = "", se = "";
        int rc;
        try
        {
            var args = new string[argc];
            for (int i = 0; i < argc; i++) args[i] = Marshal.PtrToStringUTF8(Marshal.ReadIntPtr(argv, i * IntPtr.Size)) ?? "";
            if (argc == 1 && args[0] == "--source-sha") { so = GameAppXlate.SourceSha() + "\n"; rc = 0; }
            else if (argc == 2) rc = GameAppXlate.Run(args[0], args[1], out so, out se);
            else { rc = 2; se = "usage: dmsh_xlate <job.json> <outdir>"; }
        }
        catch (Exception error) { rc = 1; se = "[RED] translator: " + error; }
        // The translator's heap is .NET's, apart from Python's: six translations at once on top of the ~2.9 GB the
        // rewrite leaves in Python's allocator reached the 6 GB ceiling and iOS ended the app at 938 of 21,990 shaders
        // (2026-11-18, no signal logged: a memory kill). Every 40 translations, and at every one while less than
        // 1.5 GB is left, a compacting collection hands the freed pages back to the system.
        var n = Interlocked.Increment(ref xlateCount);
        if (n % 40 == 0 || AvailableMiBValue() is long left && left >= 0 && left < 1536)
            GC.Collect(GC.MaxGeneration, GCCollectionMode.Aggressive, true, true);
        // CoTaskMem is malloc on this platform: the shim frees these with free().
        if (stdoutOut != IntPtr.Zero) Marshal.WriteIntPtr(stdoutOut, Marshal.StringToCoTaskMemUTF8(so ?? ""));
        if (stderrOut != IntPtr.Zero) Marshal.WriteIntPtr(stderrOut, Marshal.StringToCoTaskMemUTF8(se ?? ""));
        return rc;
    }

    internal static string Documents =>
        Path.Combine(Environment.GetEnvironmentVariable("HOME") ?? ".", "Documents");

    /// The NSP an .nsz was made from, rebuilt into Documents/.nsz/ (a folder the game list does not read); null and why
    /// when it could not be made. Each rebuilt NCA is checked against its name (its SHA-256), as the content system does.
    internal static string NszToNsp(string nsz, out string why)
    {
        why = null;
        var dir = Path.Combine(Documents, ".nsz");
        try { Directory.CreateDirectory(dir); } catch (Exception error) { why = error.Message; return null; }
        var nsp = Path.Combine(dir, Path.GetFileNameWithoutExtension(nsz) + ".nsp");
        var err = new byte[512];
        int rc;
        try { rc = gameapp_nsz(nsz, nsp, err, err.Length); }
        catch (Exception error) { why = $"{error.GetType().Name}: {error.Message}"; return null; }
        if (rc == 0 && File.Exists(nsp)) return nsp;
        why = Encoding.UTF8.GetString(err).TrimEnd('\0');
        if (string.IsNullOrEmpty(why)) why = "the conversion failed";
        return null;
    }

    /// One line from the kernel: resident, peak, footprint, free space.
    internal static string MemLine()
    {
        try
        {
            var buffer = new byte[512];
            int n = gameapp_memline(buffer, buffer.Length);
            return Encoding.UTF8.GetString(buffer, 0, Math.Max(0, Math.Min(n, buffer.Length)));
        }
        catch (Exception error)
        {
            return "unavailable (" + error.GetType().Name + ")";
        }
    }

    /// os_proc_available_memory(): the ceiling iOS will still grant this process, the figure jetsam is expressed
    /// in. Printed next to resident/peak/footprint so runs can be compared (it rises when the increased-memory-limit
    /// entitlement took effect). The same ceiling as a number for the checklist: GameAppUi.CeilingMiB (MiB, or -1
    /// where there is none to show: a simulator answers 0, a missing call -1).
    internal static string AvailableMiB()
    {
        try
        {
            long available = gameapp_available();
            return available < 0 ? "unknown (os_proc_available_memory unavailable here)" : $"{available / (1024.0 * 1024):F0}";
        }
        catch (Exception error) { return "unavailable (" + error.GetType().Name + ")"; }
    }

    /// Extended Virtual Addressing in this app's own signature (1/0), and the free address space now in GiB; -1 / -1
    /// when the shim cannot be called.
    internal static (int Extended, double FreeGiB) AddressSpaceValue()
    {
        try
        {
            gameapp_address_space(out long freeTotal, out long largest);
            return (gameapp_extended_va(), freeTotal / 1073741824.0);
        }
        catch (Exception) { return (-1, -1); }
    }

    /// The increased-memory-limit entitlement in this app's own signature: 1/0, -1 unreadable or no shim.
    internal static int IncreasedMemoryValue()
    {
        try { return gameapp_increased_memory(); }
        catch (Exception) { return -1; }
    }

    /// Whether a built game can run from Documents in this install: 0 when a dlopen there was refused (the pipeline's
    /// Documents/gameapp.load-blocked) or the signature has no get-task-allow (a distribution certificate), 1 with it
    /// (a development one), -1 unknown. GameAppUi.CannotRunHere turns Play into Export as .ipa on 0.
    internal static int LoadsFromDocumentsValue()
    {
        try
        {
            if (File.Exists(Path.Combine(Documents, "gameapp.load-blocked"))) return 0;
            int g = gameapp_get_task_allow();
            return g == 1 ? 1 : g == 0 ? 0 : -1;
        }
        catch (Exception) { return -1; }
    }

    internal static long AvailableMiBValue()
    {
        try { return GameAppUi.CeilingMiB(gameapp_available()); }
        catch (Exception) { return -1; }
    }

    /// The signing identity as the shim reports it: 1 = a .p12 is held (and its size), 0 = none, anything else =
    /// unreadable. -1 when the shim could not be called at all.
    internal static (int Holding, long P12Bytes) IdentityState()
    {
        try
        {
            int holding = gameapp_identity_status(out long p12Len);
            return (holding, p12Len);
        }
        catch (Exception) { return (-1, 0); }
    }

    /// The signing identity's state, in one line. The device path signs the framework with the installer's own
    /// certificate, and that certificate is a .p12 this app must hold ITSELF: a Keychain item is readable only
    /// inside the access group it was written into, so the copy iPorterNX's builder app stores is invisible here
    /// (measured on a device install: SecItemCopyMatching -> -25300, then stage 4b rc=12). Printed at launch,
    /// because otherwise the first sign of trouble is 2.5 minutes later at the end of a full chain.
    internal static string IdentityLine()
    {
        try
        {
            int holding = gameapp_identity_status(out long p12Len);
            int where = -1;
            try { where = gameapp_identity_where(); } catch (Exception) { }
            if (holding == 1)
                return where == 2
                    ? $"signing identity: in a protected file ({p12Len} byte .p12; this install's Keychain refused it) — the chain can sign the framework"
                    : $"signing identity: in this app's Keychain ({p12Len} byte .p12) — the chain can sign the framework";
            if (holding == 0)
                return "signing identity: MISSING — tap \"Certificate (.p12)\" and pick your .p12 (the certificate of the installer "
                     + "that installed this app: SideStore's, or FlareStore's). Without it the chain rewrites everything and then fails at stage 4b: prod.keys is NOT it";
            return "signing identity: unreadable (gameapp_identity_status -> " + holding + ")";
        }
        catch (Exception error) { return "signing identity: unavailable (" + error.GetType().Name + ")"; }
    }

    /// Store the picked .p12 and its password in this app's own Keychain -- through the shim, so the item the UI
    /// reports is exactly the item the signer reads. 0 = stored; anything else is the refusal's own code.
    internal static int IdentityStore(byte[] p12, string passphrase)
    {
        try { return gameapp_identity_store_bytes(p12, p12.LongLength, passphrase ?? string.Empty); }
        catch (Exception error) { return -999 - error.HResult; }
    }

    internal static string FreeSpace()
    {
        try
        {
            var drive = new DriveInfo("/");
            return string.Format("{0:F1} GB free of {1:F0} GB",
                drive.AvailableFreeSpace / (1024.0 * 1024 * 1024), drive.TotalSize / (1024.0 * 1024 * 1024));
        }
        catch (Exception error) { return "unknown (" + error.GetType().Name + ")"; }
    }

    private static IEnumerable<string> DocumentNames(string documents)
    {
        try { return Directory.EnumerateFiles(documents).Select(Path.GetFileName).ToList(); }
        catch (Exception) { return new List<string>(); }
    }

    /// The config gameapp_pipeline.py reads: where the extraction wrote, the host dylib inside THIS bundle, and
    /// the framework to assemble, sign and load. Written by the app, not by the harness.
    internal static string ConfigText(string bundlePath, string game = null)
    {
        var documents = Documents;
        var extract = Path.Combine(documents, "extract");
        // An exported game (GameAppExport.cs): its framework, rewritten modules, game files and shader library are in
        // the bundle (read-only); settings, saves and the profile are in Documents. It boots, it never builds.
        bool player = GameExport.IsPlayer;
        if (player) { extract = Path.Combine(GameExport.BundleGame, "extract"); game = GameExport.PlayerDump; }
        var playerLines = player
            ? new[] { "shader_packs=" + Path.Combine(GameExport.BundleGame, "shaders"), "shader_build=0" }
            : new string[0];
        // A Switch 2 package: the chain reads it itself (gameapp_pipeline.extract_nup) before the profile.
        if (!player && game != null && GameAppUi.IsNup(game) && File.Exists(Path.Combine(documents, game)))
            playerLines = new[] { "nup=" + Path.Combine(documents, game) };
        var lines = new[]
        {
            "extract_out=" + extract,
            "exe_dir=" + Path.Combine(extract, "ExeFS"),
            "modules=main subsdk0 sdk",
            "patched_dir=" + Path.Combine(extract, "rewrite"),
            // the prebuilt host the assembler grafts the game onto. Device build: the loadable DEVICE host
            // (platform 2, engine 75/75). Simulator build: the loadable simulator host. Same file name either
            // way, so the pipeline needs to know nothing about which one it is.
            "host_dylib=" + Path.Combine(bundlePath, "host-prebuilt.dylib"),
            "framework_out=" + (player ? GameExport.BundleFramework : Path.Combine(documents, "DmGameProbe.framework", "DmGameProbe")),
            "profile=" + Path.Combine(documents, "profile.tsv"),
            "romfs=" + (player ? GameExport.PlayerRomfs ?? Path.Combine(extract, "RomFS.img") : Path.Combine(extract, "RomFS.img")),
            "shader_manifest=" + Path.Combine(documents, "shader_manifest"),
            // 0 = no limit: a player's game runs until they press Quit game. It was a fixed 30 s, chosen so a harness
            // run would end by itself; a game that ends 30 s after it starts is not one anyone can play. The harness
            // (gameapp/run_gameapp.sh) still sets GAMEAPP_BOOT_SECONDS, so an unattended run still ends.
            "boot_seconds=" + (Environment.GetEnvironmentVariable("GAMEAPP_BOOT_SECONDS") ?? "0"),
            // the player starts the game; GAMEAPP_BOOT_AFTER_BUILD=1 boots it at the end of the build (harness runs)
            "boot_after_build=" + (Environment.GetEnvironmentVariable("GAMEAPP_BOOT_AFTER_BUILD") ?? "0"),
            // extract/ExeFS is removed once a build has its framework and shaders (space); the harness keeps it for
            // its chain-only runs (GAMEAPP_KEEP_EXEFS=1, gameapp/run_gameapp.sh)
            "keep_exefs=" + (Environment.GetEnvironmentVariable("GAMEAPP_KEEP_EXEFS") ?? "0"),
            // The margin the chain demands before it boots the guest IN ITS OWN PROCESS. The chain's footprint at
            // the boot is what it PEAKED at (~2.9 GiB on a device, a 3365 MiB ceiling: available 520 MiB), and the
            // guest itself commits ~2 GB of its alias region before the first frames — measured on the simulator,
            // where the guest log walks `alias region mapped up to +1.00 … +2.01 GB` around `>>> nnMain`. A floor
            // under that would invite exactly the kill the 2026-11-17 device run died of, so it sits above the
            // guest's appetite, and a device will therefore normally refuse and ask for a fresh launch instead
            // (Documents/gameapp.boot-only, ~3.3 GiB available).
            "boot_floor_mib=" + (Environment.GetEnvironmentVariable("GAMEAPP_BOOT_FLOOR_MIB") ?? "2500"),
            // Who built this app (the build script's id, GameAppBuild.cs) and from which dump: the pipeline stamps
            // the framework with them once it is signed (Documents/gameapp.framework), and the screen offers "Boot
            // again (no rebuild)" only for a framework THIS build made. `game` is empty for a run with no extraction.
            "app_build=" + GameAppBuild.Id,
            // The translation chain the host was built with (engine/dm/gfx/nvn/dmsh_chain.h): a shader pack of another
            // chain is refused by the host, and the pipeline says so by name before the boot.
            "shader_chain=" + GameAppBuild.ShaderChain,
            "game=" + (game ?? ""),
            // The update and DLC this build takes (stamped with the framework: one added later asks for a rebuild).
            "content=" + (game == null || player ? "" : GameAppUi.ContentSignature(game, DocumentNames(documents))),
        };
        return string.Join("\n", lines.Concat(playerLines)) + "\n";
    }
}

[Register("GameAppAppDelegate")]
public class GameAppAppDelegate : UIResponder, IUIApplicationDelegate
{
    [Export("application:didFinishLaunchingWithOptions:")]
    public bool FinishedLaunching(UIApplication application, NSDictionary launchOptions) => true;

    [Export("application:configurationForConnectingSceneSession:options:")]
    public UISceneConfiguration GetConfiguration(UIApplication application, UISceneSession session,
                                                 UISceneConnectionOptions options)
        => session.Role == UIWindowSceneSessionRole.ExternalDisplayNonInteractive
            ? new UISceneConfiguration("ExternalDisplay", session.Role)   // a TV / monitor (GameAppExternal.cs)
            : new UISceneConfiguration("GameApp", session.Role);
}

[Register("GameAppSceneDelegate")]
public class GameAppSceneDelegate : UIResponder, IUIWindowSceneDelegate
{
    [Export("window")] public UIWindow Window { get; set; }

    [Export("scene:willConnectToSession:options:")]
    public void WillConnect(UIScene scene, UISceneSession session, UISceneConnectionOptions options)
    {
        Window = new UIWindow((UIWindowScene) scene);
        Window.OverrideUserInterfaceStyle = UIUserInterfaceStyle.Dark;   // the Ambient design is a night one (gameapp/GameAppTheme.cs)
        Window.RootViewController = new GameAppViewController();
        Window.MakeKeyAndVisible();
        GameAppExternal.Log = text => Console.WriteLine("[external] " + text);
        // -GameAppDebugTv: no display can be attached to a simulator from the command line, so a window over the phone's own
        // scene plays it (GameAppExternal.UseDebugDisplay); -GameAppDebugGame shows the game window with a test layer, no
        // game booted. Neither is reachable without the launch argument.
        var args = NSProcessInfo.ProcessInfo.Arguments;
        if (Array.IndexOf(args, "-GameAppDebugTv") >= 0) GameAppExternal.UseDebugDisplay((UIWindowScene) scene);
        if (Array.IndexOf(args, "-GameAppDebugGame") >= 0)
            NSTimer.CreateScheduledTimer(0.8, _ => GameAppDebugGame.Show(Window.RootViewController));
    }
}

/// The tool's output, forwarded into the log view verbatim (the engine's Python parses these same lines).
public sealed class GameAppLogWriter : TextWriter
{
    private readonly Action<string> append;
    public GameAppLogWriter(Action<string> append) { this.append = append; }
    public override Encoding Encoding => Encoding.UTF8;
    public override void Write(string text) { if (!string.IsNullOrEmpty(text)) append(text); }
    public override void WriteLine(string line) { append((line ?? "") + "\n"); }
    public override void Write(char value) { append(value.ToString()); }
}

/// The game's window: a UIView whose layer IS a CAMetalLayer, configured as the engine's own front end configures its
/// (dm/ios/Sources/front_ios.m, DmGameView): BGRA8, not framebuffer-only, opaque, a drawable at the pixels of the screen it
/// is on. The engine presents the guest's frames into this layer (front_metal_layer, handed over by gameapp_shim.c);
/// before this view existed the host had no layer, read "no window", and drew nothing -- the device run of 2026-11-17
/// was heard and never seen.
[Register("GameAppGameView")]
public sealed class GameAppGameView : UIView
{
    [Export("layerClass")]
    public static Class LayerClass() => new Class(typeof(CAMetalLayer));

    public GameAppGameView(CGRect frame) : base(frame)
    {
        var layer = (CAMetalLayer) Layer;
        layer.Device = MTLDevice.SystemDefault;
        layer.PixelFormat = MTLPixelFormat.BGRA8Unorm;
        layer.FramebufferOnly = false;
        layer.Opaque = true;
        BackgroundColor = UIColor.Black;
        // A drawable size before the view is on screen (LayoutSubviews sets the real one): the engine reads the layer's
        // size at its first present, and the view reaches a window about a second after the boot starts.
#pragma warning disable CA1422 // UIScreen.MainScreen: only the fallback until the window scene gives its own screen
        var screen = UIScreen.MainScreen;
#pragma warning restore CA1422
        var scale = (nfloat) Math.Max(1.0, (double) screen.NativeScale);
        var longSide = (nfloat) Math.Max(screen.Bounds.Width, screen.Bounds.Height);
        var shortSide = (nfloat) Math.Min(screen.Bounds.Width, screen.Bounds.Height);
        layer.ContentsScale = scale;
        layer.DrawableSize = new CGSize(longSide * scale, shortSide * scale);
    }

    public CAMetalLayer MetalLayer => (CAMetalLayer) Layer;

    /// 0: the pixels of the screen the view is on. An external display caps the PICTURE's pixels across (GameAppExternal.cs).
    public double MaxPixelWidth;

    public override void MovedToWindow()
    {
        base.MovedToWindow();
        SetNeedsLayout();   // the other screen's scale (the phone's or the display's)
    }

    // Render at the pixels of the screen the view is on, as front_ios.m does.
    public override void LayoutSubviews()
    {
        base.LayoutSubviews();
        var layer = MetalLayer;
        var screen = Window?.WindowScene?.Screen;
        var scale = screen != null ? screen.NativeScale : TraitCollection.DisplayScale;
        if (scale < 1) scale = 1;
        if (MaxPixelWidth > 0 && Bounds.Width > 0 && Bounds.Width * scale > MaxPixelWidth) scale = (nfloat) (MaxPixelWidth / Bounds.Width);
        layer.ContentsScale = scale;
        layer.DrawableSize = new CGSize(Bounds.Width * scale, Bounds.Height * scale);
    }
}

/// The game on screen: full screen, landscape. The picture sits in the rectangle the engine's own layout gives it
/// (16:9, or the shape the game presents), and the engine's touch gamepad covers the whole screen over it
/// (DmGamepad.m, linked into the host by load/prebuild_loadable.sh; load/front_pad.m): 2D / 3D / Tap layouts, physical
/// controllers too. Its gear opens the controller layout and "Quit game". Quit asks first (a stray tap in a game is
/// common), then ends the guest through the shim: the same SIGTERM the time limit sends and the host's graceful exit,
/// with its own line in the chain log. The gamepad lives in the host, so it can only be made once the boot has
/// dlopen'd the framework: the view asks every half second until it has one. A host with no gamepad (an older build)
/// gets the plain "Quit game" button instead, shown on a tap.
public sealed class GameAppGameViewController : UIViewController
{
    private readonly GameAppGameView game;
    private readonly UIButton quit = UIButton.FromType(UIButtonType.System);
    private NSTimer hideTimer;
    private NSTimer padTimer;
    private UIView pad;
    private bool padRefused;   // the host has no gamepad: the fallback Quit button stays in charge
    private readonly List<NSObject> observers = new List<NSObject>();
    private bool onDisplay;    // the picture is on an external display: this screen is the controller (GameAppExternal.cs)
    private UIView tvCover;    // "Playing on the TV": over the pad while a hardware controller plays, or before the pad exists
    private UIButton tvMenu;   // the cover's own menu button (the engine's pad hides its gear while a controller plays)
    private UILabel tvSub;
    /// Documents/games/<id>/settings.tsv of the game being started (GameAppExternal.PlayerWantsTv); null: the environment only.
    public string SettingsFile;

    public GameAppGameViewController()
    {
        game = new GameAppGameView(new CGRect(0, 0, 100, 100));
        ModalPresentationStyle = UIModalPresentationStyle.FullScreen;
    }

    /// The CAMetalLayer the host presents into. Loads the view, so the layer exists before the framework is dlopen'd.
    public IntPtr LayerHandle
    {
        get { _ = View; return game.MetalLayer.Handle; }
    }

    public override void LoadView()
    {
        // ClipsToBounds: with "Fill screen" (DM_PICTURE_FIT=fill) the picture is larger than the screen and its edges are cropped
        View = new UIView { BackgroundColor = UIColor.Black, ClipsToBounds = true };
        View.AddSubview(game);
    }

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        quit.SetTitle("Quit game", UIControlState.Normal);
        quit.SetTitleColor(UIColor.White, UIControlState.Normal);
        quit.TitleLabel.Font = UIFont.BoldSystemFontOfSize(15);
        quit.BackgroundColor = UIColor.FromWhiteAlpha(0.0f, 0.55f);
        quit.Layer.CornerRadius = 14;
        quit.ContentEdgeInsets = new UIEdgeInsets(8, 16, 8, 16);
        quit.AccessibilityIdentifier = "quit-game";
        quit.TranslatesAutoresizingMaskIntoConstraints = false;
        quit.AddTarget((_, _) => AskToQuit(), UIControlEvent.TouchUpInside);
        View.AddSubview(quit);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            quit.TopAnchor.ConstraintEqualTo(View.SafeAreaLayoutGuide.TopAnchor, 12),
            quit.LeadingAnchor.ConstraintEqualTo(View.SafeAreaLayoutGuide.LeadingAnchor, 12),
        });
        View.AddGestureRecognizer(new UITapGestureRecognizer(() => { if (pad == null) ShowQuit(); }));
        ShowQuit();   // visible until the gamepad is up, so the way out is never a thing to hunt for
        // The gear's "Quit game" (load/front_pad.m dm_game_menu) and a new picture shape (front_picture_shape).
        observers.Add(NSNotificationCenter.DefaultCenter.AddObserver(new NSString("GameAppQuitRequested"), _ => AskToQuit()));
        observers.Add(NSNotificationCenter.DefaultCenter.AddObserver(new NSString("DmFrontPictureShape"), _ => { View.SetNeedsLayout(); TvView?.SetNeedsLayout(); }));
        // A hardware controller coming or going changes what the phone shows while the picture is on a TV
        observers.Add(NSNotificationCenter.DefaultCenter.AddObserver(new NSString("GCControllerDidConnectNotification"), _ => UpdateTvCover()));
        observers.Add(NSNotificationCenter.DefaultCenter.AddObserver(new NSString("GCControllerDidDisconnectNotification"), _ => UpdateTvCover()));
    }

    public override void ViewDidAppear(bool animated)
    {
        base.ViewDidAppear(animated);
        GameAppExternal.Begin(this, GameAppExternal.PlayerWantsTv(SettingsFile));   // a display connected now or later takes the picture
        TryMakePad();
        if (pad == null && !padRefused && padTimer == null)
            padTimer = NSTimer.CreateRepeatingScheduledTimer(0.5, _ => TryMakePad());
    }

    private void TryMakePad()
    {
        if (pad != null || padRefused) { padTimer?.Invalidate(); padTimer = null; return; }
        if (onDisplay) UpdateTvCover();   // the menu button needs the host, which the boot loads a moment after the window
        var bounds = View.Bounds;
        var handle = GameAppMain.gameapp_pad_create(bounds.Width, bounds.Height, out var why);
        if (handle == IntPtr.Zero)
        {
            if (why == 2) { padRefused = true; padTimer?.Invalidate(); padTimer = null; }
            return;   // 1: the boot has not dlopen'd the framework yet; the timer asks again
        }
        padTimer?.Invalidate(); padTimer = null;
        pad = ObjCRuntime.Runtime.GetNSObject<UIView>(handle, true);
        if (game.Superview == View) View.InsertSubviewAbove(pad, game); else View.InsertSubview(pad, 0);   // the picture may be on a TV
        hideTimer?.Invalidate();
        quit.Hidden = true;   // the gear's menu has "Quit game" now
        UpdateTvCover();
        View.SetNeedsLayout();
    }

    public override void ViewDidLayoutSubviews()
    {
        base.ViewDidLayoutSubviews();
        var b = View.Bounds;
        var insets = View.SafeAreaInsets;
        if (onDisplay)
        {
            // The picture is on the display: the pad takes the whole phone screen (load/front_tv.m, the engine's onTV layout)
            if (pad != null) GameAppHost.PadLayoutTv((IntPtr) pad.Handle, b.Width, b.Height);
            if (tvCover != null) tvCover.Frame = b;
            return;
        }
        var rect = new double[4];
        if (GameAppMain.gameapp_pad_layout(pad != null ? (IntPtr) pad.Handle : IntPtr.Zero, b.Width, b.Height,
                                           insets.Top, insets.Left, insets.Bottom, insets.Right, rect) != 0
            && rect[2] > 0 && rect[3] > 0)
        {
            game.Frame = new CGRect(rect[0], rect[1], rect[2], rect[3]);
            return;
        }
        // Before the host is loaded: a 16:9 picture, fitted and centred, which is what the engine starts with.
        var w = Math.Min((double) b.Width, (double) b.Height * 16.0 / 9.0);
        var h = w * 9.0 / 16.0;
        game.Frame = new CGRect((b.Width - w) / 2, (b.Height - h) / 2, w, h);
    }

    // ---- the picture on an external display (GameAppExternal.cs) ------------------------------------------------------

    private UIView TvView => onDisplay ? game.Superview : null;

    // for gameapp/GameAppExternal.cs GameAppDebugGame (-GameAppDebugGame): the layer to draw a test picture into
    internal CAMetalLayer MetalLayerForDebug => game.MetalLayer;
    internal UIScreen GameScreenForDebug => game.Window?.WindowScene?.Screen;

    /// The picture's view goes to the display's window; this screen becomes the controller, in either orientation.
    public void MovePictureTo(UIView host, UIScreen screen, double maxPixels)
    {
        onDisplay = true;
        game.MaxPixelWidth = maxPixels;
        game.RemoveFromSuperview();
        host.AddSubview(game);
        host.SetNeedsLayout();
        UpdateTvCover();
        SetNeedsUpdateOfSupportedInterfaceOrientations();
        View.SetNeedsLayout();
    }

    /// The display is gone: the picture returns under the pad, and the phone is landscape again.
    public void MovePictureBack()
    {
        if (!onDisplay) return;
        onDisplay = false;
        game.MaxPixelWidth = 0;
        game.RemoveFromSuperview();
        View.InsertSubview(game, 0);
        tvCover?.RemoveFromSuperview();
        if (pad != null) View.SetNeedsLayout();
        else ShowQuit();
        SetNeedsUpdateOfSupportedInterfaceOrientations();
        View.SetNeedsLayout();
    }

    /// The picture's rectangle in the display's window: the engine's own layout for its size (the picture's shape, "Fill"
    /// included), or 16:9 fitted before the host is loaded.
    public void LayoutPictureOnDisplay(CGRect b)
    {
        var rect = new double[4];
        if (GameAppMain.gameapp_pad_layout(IntPtr.Zero, b.Width, b.Height, 0, 0, 0, 0, rect) != 0 && rect[2] > 0 && rect[3] > 0)
        {
            game.Frame = new CGRect(rect[0], rect[1], rect[2], rect[3]);
            return;
        }
        var w = Math.Min((double) b.Width, (double) b.Height * 16.0 / 9.0);
        var h = w * 9.0 / 16.0;
        game.Frame = new CGRect((b.Width - w) / 2, (b.Height - h) / 2, w, h);
    }

    private static bool HardwareController()
    {
        foreach (var c in GameController.GCController.Controllers) if (c.ExtendedGamepad != null) return true;
        return false;
    }

    /// "Playing on the TV" over the pad while a hardware controller plays (the engine's pad hides its controls AND its gear
    /// then, in this layout) or before the pad exists; the menu button on it is the host's own in-game menu.
    private void UpdateTvCover()
    {
        var show = onDisplay && (HardwareController() || pad == null);
        if (!show) { tvCover?.RemoveFromSuperview(); return; }
        if (tvCover == null)
        {
            tvCover = new UIView { BackgroundColor = UIColor.FromRGB(0.04f, 0.05f, 0.09f) };
            tvCover.AutoresizingMask = UIViewAutoresizing.FlexibleDimensions;
            var icon = new UIImageView(UIImage.GetSystemImage("tv")) { TintColor = UIColor.FromWhiteAlpha(1f, 0.55f), ContentMode = UIViewContentMode.ScaleAspectFit };
            var title = new UILabel { Text = "Playing on the TV", TextColor = UIColor.White, Font = UIFont.BoldSystemFontOfSize(24), TextAlignment = UITextAlignment.Center };
            tvSub = new UILabel { TextColor = UIColor.FromWhiteAlpha(1f, 0.6f), Font = UIFont.SystemFontOfSize(15), TextAlignment = UITextAlignment.Center, Lines = 0 };
            var stack = new UIStackView(new UIView[] { icon, title, tvSub }) { Axis = UILayoutConstraintAxis.Vertical, Spacing = 10, Alignment = UIStackViewAlignment.Center, TranslatesAutoresizingMaskIntoConstraints = false };
            tvCover.AddSubview(stack);
            tvMenu = UIButton.FromType(UIButtonType.System);
            tvMenu.SetImage(UIImage.GetSystemImage("gearshape.fill"), UIControlState.Normal);
            tvMenu.TintColor = UIColor.White;
            tvMenu.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.14f);
            tvMenu.Layer.CornerRadius = 22;
            tvMenu.ShowsMenuAsPrimaryAction = true;
            tvMenu.AccessibilityIdentifier = "tv-menu";
            tvMenu.TranslatesAutoresizingMaskIntoConstraints = false;
            tvCover.AddSubview(tvMenu);
            NSLayoutConstraint.ActivateConstraints(new[]
            {
                stack.CenterXAnchor.ConstraintEqualTo(tvCover.CenterXAnchor),
                stack.CenterYAnchor.ConstraintEqualTo(tvCover.CenterYAnchor),
                stack.LeadingAnchor.ConstraintGreaterThanOrEqualTo(tvCover.LeadingAnchor, 24),
                icon.HeightAnchor.ConstraintEqualTo(56),
                tvMenu.TopAnchor.ConstraintEqualTo(tvCover.SafeAreaLayoutGuide.TopAnchor, 12),
                tvMenu.CenterXAnchor.ConstraintEqualTo(tvCover.CenterXAnchor),
                tvMenu.WidthAnchor.ConstraintEqualTo(44),
                tvMenu.HeightAnchor.ConstraintEqualTo(44),
            });
        }
        tvSub.Text = pad == null ? "The game is starting." : "Your controller is connected. The gear opens the game's menu.";
        if (tvMenu.Menu == null) tvMenu.Menu = GameAppHost.GameMenu();   // null until the host is loaded: asked again at the next update
        tvMenu.Hidden = tvMenu.Menu == null;
        tvCover.Frame = View.Bounds;
        if (tvCover.Superview != View) View.AddSubview(tvCover);
        View.BringSubviewToFront(quit);
    }

    private void ShowQuit()
    {
        if (pad != null) return;
        hideTimer?.Invalidate();
        quit.UserInteractionEnabled = true;
        UIView.Animate(0.2, () => quit.Alpha = 1);
        hideTimer = NSTimer.CreateScheduledTimer(4.0, _ =>
        {
            quit.UserInteractionEnabled = false;
            UIView.Animate(0.4, () => quit.Alpha = 0);
        });
    }

    private void AskToQuit()
    {
        if (PresentedViewController != null) return;
        hideTimer?.Invalidate();   // the button stays while the question is open
        var alert = UIAlertController.Create("End the game?",
            "The game stops and the app closes. Boot again starts it from the framework already built.",
            UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("End game", UIAlertActionStyle.Destructive,
                                             _ => GameAppMain.gameapp_quit_game()));
        alert.AddAction(UIAlertAction.Create("Keep playing", UIAlertActionStyle.Cancel, _ => ShowQuit()));
        PresentViewController(alert, true, null);
    }

    public override void ViewDidDisappear(bool animated)
    {
        base.ViewDidDisappear(animated);
        GameAppExternal.End(this);
        padTimer?.Invalidate(); padTimer = null;
        foreach (var o in observers) NSNotificationCenter.DefaultCenter.RemoveObserver(o);
        observers.Clear();
    }

    public override bool PrefersStatusBarHidden() => true;
    public override bool PrefersHomeIndicatorAutoHidden => true;
    // Edge swipes need two tries, as in the engine's own front end: a thumb on the D-pad must not leave the game.
    public override UIRectEdge PreferredScreenEdgesDeferringSystemGestures => UIRectEdge.All;
    // Landscape on the phone; the controller on a TV also works upright (the picture is not on this screen then)
    // An iPad plays in every orientation: the picture's rectangle comes from the engine's layout for whatever shape the window has (a wide
    // one has the picture centred with the controls over it, a tall one has the picture on top and the controls below, as the engine's
    // own iPad layout does), so a Split View or Stage Manager window needs no special case.
    public override UIInterfaceOrientationMask GetSupportedInterfaceOrientations()
        => Theme.IsPad ? UIInterfaceOrientationMask.All : onDisplay ? UIInterfaceOrientationMask.AllButUpsideDown : UIInterfaceOrientationMask.Landscape;
    public override UIInterfaceOrientation PreferredInterfaceOrientationForPresentation()
    {
        if (!Theme.IsPad) return UIInterfaceOrientation.LandscapeRight;
        // the iPad starts the game the way it is held
        var now = View.Window?.WindowScene?.InterfaceOrientation ?? UIInterfaceOrientation.LandscapeRight;
        return now == UIInterfaceOrientation.Unknown ? UIInterfaceOrientation.LandscapeRight : now;
    }
    public override bool ShouldAutorotate() => true;
}

public sealed partial class GameAppViewController : UIViewController
{
    // The extraction threw (P.Main raised). A distinct code so the exit line can never read "exit 0 (ok)".
    private const int ExtractionThrew = 70;
    // An .nsz that could not be rebuilt into its NSP (gameapp/nsz_decompress.c): its own code, the extractor never ran.
    private const int NszFailed = 71;
    // What the view keeps and what gameapp.ui.log may grow to. A run that died must not leave an unreadable,
    // ever-growing file (measured on the device: a tail bug grew the UI log to 511 KB while the real chain log
    // stayed 1.8 KB).
    private const int MaxViewChars = 200000;

    private readonly UIButton action = new UIButton(UIButtonType.System);   // the selected game's ONE primary button
    private readonly UIButton export = new UIButton(UIButtonType.System);
    private bool rebuildRequested;     // set by "Rebuild from scratch" / "Set up and play": the chain runs, a pending boot request is withdrawn
    private readonly UIProgressView progress = new UIProgressView(UIProgressViewStyle.Default);

    // Pages: 0 Library, 1 Cloud, 2 Keys, 3 Logs, 4 Settings, 5 Exported apps. Two tabs: Library, and Settings, which
    // opens Cloud, Keys, Logs and Exported apps as sub-pages (each with a way back). The log timeline keeps receiving
    // lines while hidden: Append and TailLogs do not know which page is showing.
    private readonly UIView[] pages = { new UIView(), new UIView(), new UIView(), new UIView(), new UIView(), new UIView(), new UIView() };
    private readonly UIButton[] tabs = { new UIButton(UIButtonType.System), new UIButton(UIButtonType.System) };
    private static readonly string[] TabTitles = { "Library", "Settings" };
    private static readonly string[] TabIcons = { "square.grid.2x2.fill", "gearshape.fill" };
    private static readonly string[] TabIds = { "tab-games", "tab-settings" };
    private int currentTab;
    private readonly UIStackView banner = new UIStackView();      // missing prerequisites, tappable (GameAppUi.Prerequisites)
    private readonly UIStackView grid = new UIStackView();        // the "All games" list
    private readonly UILabel detailName = new UILabel();
    private readonly UILabel detailSummary = new UILabel();
    private readonly UIStackView keysRows = new UIStackView();    // the Keys page's checklist rows
    private string selected;           // the game file name the library's detail is about
    private Timer ticker;
    private Timer tailer;
    private DateTime startedAt;
    private bool running;
    private bool geometryWritten;
    private long chainOffset, stagesOffset;
    private string chosen;
    private string lastStage;          // the chain's current step, shown on the run card and the build screen (gameapp.stage)
    private string lastStageLogged;    // the same, so a step is appended to the log exactly once
    private string lastTailWindow;     // the last chunk the tailer appended, so an identical one is never replayed
    private GameAppGameViewController gameWindow;   // the game's full-screen window, made at BeginRun, shown at the boot
    private bool gameWindowShown;

    // The tool's own screen is a portrait one on the phone; only the game view is landscape (GameAppGameViewController). An iPad turns
    // every way: the library and Settings have a layout for each shape (ApplyAdaptiveLayout).
    public override UIInterfaceOrientationMask GetSupportedInterfaceOrientations() => Theme.IsPad ? UIInterfaceOrientationMask.All : UIInterfaceOrientationMask.Portrait;

    /// The game view and its layer, made BEFORE the chain starts: the shim hands the layer to the host right after the
    /// framework is dlopen'd, so it has to exist by then. Main thread (UIKit). The view is put on screen later, when the
    /// pipeline reaches its boot stage (ShowGameWindowAtBoot): over the rewrite it would hide the progress.
    private void PrepareGameWindow()
    {
        try
        {
            gameWindow = new GameAppGameViewController();
            var rel = chosen == null ? null : GameAppUi.SettingsRelativePath(chosen);
            gameWindow.SettingsFile = rel == null ? null : Path.Combine(GameAppMain.Documents, rel);
            GameAppMain.gameapp_set_game_layer(gameWindow.LayerHandle);
            gameWindowShown = false;
        }
        catch (Exception error)
        {
            gameWindow = null;
            Write($"could not make the game view: {error.GetType().Name}: {error.Message} — the game will run with no window");
        }
    }

    /// Put the game on screen once the pipeline says it is booting the guest (gameapp.stage, GameAppUi.IsBootStage).
    private void ShowGameWindowAtBoot()
    {
        if (!running || gameWindow == null || gameWindowShown || !GameAppUi.IsBootStage(lastStage)) return;
        gameWindowShown = true;
        UIViewController top = this;
        while (top.PresentedViewController != null) top = top.PresentedViewController;
        top.PresentViewController(gameWindow, false, () => Write("game view on screen (landscape, full screen)"));
        UIApplication.SharedApplication.IdleTimerDisabled = true;   // the screen must not sleep under a game
    }

    private void DismissGameWindow()
    {
        if (gameWindow == null) return;
        if (gameWindowShown) gameWindow.DismissViewController(false, null);
        UIApplication.SharedApplication.IdleTimerDisabled = false;
        gameWindow = null;
        gameWindowShown = false;
    }

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        GameAppUi.DeviceName = Theme.IsPad ? "iPad" : "iPhone";   // the words of the screen, before any of it is built
        View.BackgroundColor = Theme.Ground;
        ambient.Frame = View.Bounds;
        ambient.AutoresizingMask = UIViewAutoresizing.FlexibleDimensions;
        View.AddSubview(ambient);
        PrepareLog();
        LoadCloud();
        LoadBuildQueue();

        // Export: share the LOG FILES themselves through UIActivityViewController, so a run that misbehaved can
        // be AirDropped or mailed out without copying text off the screen -- which the user could not do when the
        // app stopped responding. The logs are kept append-only across launches, so a run that died still has its
        // evidence on disk to share.
        export.AccessibilityIdentifier = "export-logs";
        export.TouchUpInside += (_, __) => ExportLogs();

        // An exported game sets up its Documents (settings, saves, profile, the framework stamp) before the first screen.
        GameExport.PreparePlayerDocuments(GameAppMain.Documents, text => Write(text));
        BuildGamesPage();
        BuildCloudPage();
        BuildKeysPage();
        BuildLogsPage();
        BuildSettingsPage();
        BuildAppsPage();
        BuildQueuePage();
        if (GameExport.IsPlayer)
            BuildPlayerPage();   // an exported game: its own one screen, no tabs
        else
        {
            BuildTabBar();
            ShowTab(0);
            AddKeyCommands();
        }

        Write($"device: {UIDevice.CurrentDevice.Model} — {UIDevice.CurrentDevice.SystemName} {UIDevice.CurrentDevice.SystemVersion}");
        Write($"memory at launch: {GameAppMain.MemLine()}");
        Write($"memory ceiling at launch: available {GameAppMain.AvailableMiB()} MiB (os_proc_available_memory — the number jetsam enforces; it rises only if the increased-memory-limit entitlement is applied)");
        Write($"free space: {GameAppMain.FreeSpace()} — Documents: {GameAppMain.Documents}");
        Write(GameAppMain.IdentityLine());
        var argsPath = Path.Combine(GameAppMain.Documents, "gameapp.args");
        bool hadArgs = File.Exists(argsPath);
        bool interrupted = ReportInterruptedRunIfAny();
        RefreshGames();
        RefreshScreen();
        StartWarmAtLaunch();   // GameAppWarm.cs: an exported player prepares its own Metal cache
        // Back from the Files app or the background, Documents may have gained a game or a key: say so.
        NSNotificationCenter.DefaultCenter.AddObserver(UIApplication.WillEnterForegroundNotification,
                                                       _ => { RefreshScreen(); RefreshCloud(null); });
        // The cloud listing and its icons wait for the first frame (ViewDidAppear, GameAppLibrary.cs): the library
        // is drawn from what is on disk, and the network is not on the way to it.
        Write($"launch: view built in {launchClock.ElapsedMilliseconds} ms");

        // The tailer reads the append-only logs from their END, so the log and the run card describe THIS
        // launch. Otherwise a launch that runs nothing would replay the previous run's whole log into the screen.
        chainOffset = LengthOf(Path.Combine(GameAppMain.Documents, "gameapp.chain.log"));
        stagesOffset = LengthOf(Path.Combine(GameAppMain.Documents, "pipeline.stages.log"));

        // The chain runs in CPython: its stdout/stderr go to Documents/gameapp.chain.log, its stage lines to
        // Documents/pipeline.stages.log. Tail both into the log.
        tailer = new Timer(_ => BeginInvokeOnMainThread(TailLogs), null, TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(1));

        // One run per explicit user action. Documents/gameapp.args is a ONE-SHOT request (the harness leaves the
        // extraction argv there; tapping "Set up and play" writes it too). If the PREVIOUS run never finished
        // (gameapp.running is still there) this launch must NOT start another: after a kill an auto-run on every
        // launch became a loop of ~3.5 GiB attempts (docs/MEMORY.md).
        if (interrupted)
        {
            if (hadArgs)
                Write("gameapp.args present, but the previous run was interrupted — not starting again; tap Set up and play on a game to start one");
        }
        else if (File.Exists(argsPath))
        {
            Write("gameapp.args present — starting once (one launch, one process)");
            AutoRunFromArgs(argsPath);
        }
        // The pipeline's other request: it ended the last process on purpose before stage 3 (gameapp.continue, GameAppResume.cs).
        else if (File.Exists(ContinuePath))
        {
            ContinueBuildAtLaunch();
        }
        // The chain's OWN request, the only run this app starts without a tap: when the chain refuses to boot
        // in-process (its footprint leaves the guest ~1 GiB of a 3365 MiB ceiling) it writes
        // Documents/gameapp.boot-only and ends cleanly; the next launch boots it in a process that has rewritten
        // nothing. A reopen after a CLEAN end finishes that run; a reopen after a kill is not a request (the
        // `interrupted` branch above), so this can never become a retry loop.
        else if (File.Exists(BootOnlyPath))
        {
            // Two states (gameapp_pipeline.py, BOOT_ONLY_ATTEMPTED): ARMED, so this launch starts it; ATTEMPTED, a
            // boot began from it and never came back, so it is not started again on sight. A tap on the main
            // button still boots it (a dlopen, not a rebuild); the rebuild button is the other way.
            string request = null;
            try { request = File.ReadAllText(BootOnlyPath); }
            catch (Exception error) { Write($"gameapp.boot-only is unreadable: {error.GetType().Name}: {error.Message} — not starting; tap Play to boot the framework"); }
            if (request != null && GameAppUi.CannotRunHere(GatherFacts()))
            {
                // A distribution certificate: the boot would be refused at the dlopen ("file system sandbox blocked
                // mmap()"), and starting it at every launch was the "iBuilder starts again" loop (2026-11-18).
                try { File.Delete(BootOnlyPath); } catch (Exception) { }
                Write("gameapp.boot-only withdrawn: this install's certificate (distribution, no get-task-allow) cannot run a game "
                      + "from Documents — use Export as .ipa and install the game as its own app");
                RefreshScreen();
            }
            else if (request != null && GatherFacts().Framework != FrameworkState.Fresh)
            {
                // The request points at a framework THIS version of the app did not build (an older build's, or none):
                // booting it would run an older host and rewrite, so the request is withdrawn, never started.
                try
                {
                    File.Delete(BootOnlyPath);
                    Write("gameapp.boot-only present, but the framework on disk was not built by this version of the app — withdrawn, nothing is booted; tap Set up and play on a game to rebuild");
                }
                catch (Exception error) { Write($"could not withdraw {BootOnlyPath}: {error.GetType().Name}: {error.Message}"); }
                RefreshScreen();
            }
            else if (request != null)
            {
                // GameAppUi.ClassifyBootRequest reads the first word (BOOT_ONLY_ATTEMPTED in gameapp_pipeline.py
                // writes it, in place, right before the boot). The main button offers the boot in both states.
                if (GameAppUi.ClassifyBootRequest(true, request) == BootRequestState.Attempted)
                {
                    Write("gameapp.boot-only present, but a boot from it was started and never came back — not starting again; tap Play to boot the framework once more (nothing is rebuilt)");
                }
                else
                {
                    Write("gameapp.boot-only present — the framework is built and signed; this launch boots it (~3.3 GiB free, nothing is rebuilt)");
                    BeginRun("boot-only (the framework is built and signed)", null, skipExtract: true, bootOnly: true);
                }
            }
        }
    }

    private bool autoStarted;

    /// Documents/gameapp.boot-only: the chain's request that stage 5 be run in a process that has run NO other
    /// stage of the chain. The pipeline consumes it at chain-start (and re-derives nothing), and this app writes
    /// it for the boot-only path. It is also what makes a TAP cheap: tapping a game while this is pending boots
    /// instead of re-running the ~2.5-minute chain, which would only end by refusing the boot and writing the same
    /// request again.
    private static string BootOnlyPath => Path.Combine(GameAppMain.Documents, "gameapp.boot-only");

    /// The harness (or a user tapping Build & run) leaves the extraction argv in Documents/gameapp.args: run it
    /// without a tap, so "one launch goes from the game in Documents to nnMain" is a launch and nothing else. A
    /// single line `chain-only` skips the extraction and chains on files already in Documents/extract (the
    /// re-measurement path oneproc/run_oneproc.sh has as --from-build).
    ///
    /// The file is a ONE-SHOT request and a HINT, never a set of paths to trust. It is consumed (renamed) before
    /// the run starts, so a relaunch -- including one after a kill -- cannot start a second run from it; and every
    /// entry is re-anchored under the CURRENT Documents, because an app reinstall gives the container a new UUID
    /// and a path written by the previous install then points at a container that no longer exists (measured:
    /// `DirectoryNotFoundException: IO_PathNotFound_Path, /…/Documents/prod.keys` with the OLD UUID).
    private void AutoRunFromArgs(string argsPath)
    {
        if (autoStarted) return;
        autoStarted = true;
        try
        {
            var lines = File.ReadAllLines(argsPath).Where(l => !string.IsNullOrEmpty(l)).ToList();
            ConsumeArgs(argsPath);   // one-shot: the request is used once, then it cannot fire again
            if (lines.Count == 1 && lines[0] == "chain-only")
            {
                BeginRun("chain-only (files already in Documents/extract)", null, skipExtract: true);
                return;
            }
            if (lines.Count == 1 && lines[0] == "boot-only")
            {
                // What the chain leaves behind when it refuses to boot in-process: the framework is built and
                // signed, and this launch — which has rewritten nothing — has the memory to run the guest.
                BeginRun("boot-only (the framework is built and signed)", null, skipExtract: true, bootOnly: true);
                return;
            }
            if (lines.Count >= 5)
            {
                // The extraction argv is fixed-shape: <keys> <base.nsp> <update|-> <out dir> [flags]. Re-resolve
                // the four path-shaped entries against the CURRENT Documents; the flag is passed through.
                var arguments = new[]
                {
                    ResolveUnderDocuments(lines[0]),
                    ResolveUnderDocuments(lines[1]),
                    lines[2] == "-" ? "-" : ResolveUnderDocuments(lines[2]),
                    ResolveUnderDocuments(lines[3]),
                    lines[4],
                };
                if (arguments[0] != lines[0] || arguments[1] != lines[1] || arguments[3] != lines[3])
                    Write("gameapp.args: paths re-resolved under the current Documents (the stored ones were stale)");
                BeginRun(Path.GetFileName(arguments[1]), arguments, skipExtract: false);
            }
            else { Write($"gameapp.args has {lines.Count} lines; expected the extraction argv (5 lines) or 'chain-only'"); }
        }
        catch (Exception error) { Write($"gameapp.args: {error.GetType().Name}: {error.Message}"); }
    }

    /// Rename the one-shot request aside (kept as evidence) so no relaunch can run it again; fall back to
    /// deleting it if the rename fails.
    private static void ConsumeArgs(string argsPath)
    {
        try { File.Move(argsPath, argsPath + ".done", overwrite: true); }
        catch { try { File.Delete(argsPath); } catch { } }
    }

    /// Re-anchor a stored path or name under the CURRENT Documents. Anything remembered across launches is a
    /// hint -- only the last path component is kept -- never a path to trust.
    private static string ResolveUnderDocuments(string hint)
    {
        if (string.IsNullOrEmpty(hint) || hint == "-") return hint;
        var name = Path.GetFileName(hint.TrimEnd(Path.DirectorySeparatorChar));
        return Path.Combine(GameAppMain.Documents, name);
    }

    /// The inverse: store only the name (relative to Documents) so the request is portable across installs.
    private static string RelativeToDocuments(string path)
    {
        if (string.IsNullOrEmpty(path)) return path;
        var documents = GameAppMain.Documents;
        if (path.StartsWith(documents + Path.DirectorySeparatorChar, StringComparison.Ordinal))
            return path.Substring(documents.Length + 1);
        return Path.GetFileName(path);
    }

    public override void ViewDidLayoutSubviews()
    {
        base.ViewDidLayoutSubviews();
        ApplyAdaptiveLayout();
        if (geometryWritten || View.Bounds.Width <= 0) return;
        geometryWritten = true;
        var safe = View.SafeAreaInsets;
        Write(string.Create(System.Globalization.CultureInfo.InvariantCulture,
            $"layout: view {View.Bounds.Width:F0}x{View.Bounds.Height:F0}, safe insets t{safe.Top:F0} l{safe.Left:F0} b{safe.Bottom:F0} r{safe.Right:F0}"));
    }

    private IEnumerable<string> Dumps()
    {
        var documents = GameAppMain.Documents;
        if (!Directory.Exists(documents)) yield break;
        var extensions = new[] { ".nsp", ".xci", ".nsz", ".nup" };   // .nup: a Switch 2 game (gameapp_pipeline.extract_nup); .xcz: nothing here rebuilds it yet
        foreach (var file in Directory.EnumerateFiles(documents).OrderBy(f => f))
            if (extensions.Contains(Path.GetExtension(file).ToLowerInvariant()) && Path.GetFileName(file) != "prod.keys")
                yield return file;
    }

    private void RefreshGames()
    {
        var keys = Path.Combine(GameAppMain.Documents, "prod.keys");
        Write(File.Exists(keys) ? "prod.keys: found in Documents"
                                : "prod.keys: MISSING. Use Import (or the Files app) to put it in Documents.");
        var found = Dumps().ToList();
        Write($"games found: {found.Count}");
        foreach (var game in found) Write($"  {Path.GetFileName(game)}");
    }

    /// What the screen's decisions depend on, read from Documents and the Keychain right now. Everything that
    /// decides what to show or what the main button does is in GameAppUi (tested on a Mac); this only looks.
    private UiFacts GatherFacts()
    {
        var bootExists = File.Exists(BootOnlyPath);
        string bootContent = null;
        if (bootExists)
        {
            // Unreadable stays null, which GameAppUi.ClassifyBootRequest reads as attempted: the reading that
            // never starts anything by itself. The launch path says why it is unreadable.
            try { bootContent = File.ReadAllText(BootOnlyPath); }
            catch (Exception) { bootContent = null; }
        }
        var identity = GameAppMain.IdentityState();

        // The framework on disk, and whether THIS build made it (Documents/gameapp.framework, written by the pipeline
        // right after it signs). The paths are the config's: framework_out and extract/flat.
        var frameworkOnDisk = GameExport.IsPlayer
            ? File.Exists(GameExport.BundleFramework) && Directory.Exists(Path.Combine(GameExport.BundleGame, "extract", "flat"))
            : File.Exists(Path.Combine(GameAppMain.Documents, "DmGameProbe.framework", "DmGameProbe"))
              && Directory.Exists(Path.Combine(GameAppMain.Documents, "extract", "flat"));
        string stamp = null;
        if (frameworkOnDisk)
        {
            // No stamp (or an unreadable one) stays null: GameAppUi.ClassifyFramework reads that as stale.
            try { stamp = File.ReadAllText(FrameworkStampPath); }
            catch (Exception) { stamp = null; }
        }
        var addressSpace = GameAppMain.AddressSpaceValue();
        return new UiFacts(
            Running: running,
            HasKeys: File.Exists(Path.Combine(GameAppMain.Documents, "prod.keys")),
            Games: GameExport.IsPlayer ? new List<string> { GameExport.PlayerDump }   // an exported game: its one game
                                       : GameAppUi.PlayableGames(Dumps().Select(Path.GetFileName)),   // updates and add-ons ride with their base game
            PlayerMode: GameExport.IsPlayer,
            LoadsFromDocuments: GameAppMain.LoadsFromDocumentsValue(),
            IdentityState: identity.Holding,
            P12Bytes: identity.P12Bytes,
            AvailableMiB: GameAppMain.AvailableMiBValue(),
            ExtendedAddressing: addressSpace.Extended,
            AddressSpaceFreeGiB: addressSpace.FreeGiB,
            IncreasedMemory: GameAppMain.IncreasedMemoryValue(),
            BootRequest: GameAppUi.ClassifyBootRequest(bootExists, bootContent),
            Framework: GameAppUi.ClassifyFramework(frameworkOnDisk, stamp, GameAppBuild.Id),
            FrameworkGame: GameAppUi.ParseFrameworkStamp(stamp)?.Game,
            FrameworkContent: GameAppUi.ParseFrameworkStamp(stamp)?.Content,
            ShaderPacks: InstalledShaderPacks(),
            PendingShaderPacks: Directory.Exists(GameAppMain.Documents)
                ? Directory.EnumerateFiles(GameAppMain.Documents, "*.shaders.tar")
                           .Select(p => Path.GetFileName(p).Split('.')[0].ToUpperInvariant()).ToList()
                : new List<string>());
    }

    /// Documents/shaders/<id>/ folders holding a shader map: the packs gameapp_pipeline.install_shader_packs installed.
    private static List<string> InstalledShaderPacks()
    {
        var root = GameExport.IsPlayer ? Path.Combine(GameExport.BundleGame, "shaders") : Path.Combine(GameAppMain.Documents, "shaders");
        if (!Directory.Exists(root)) return new List<string>();
        return Directory.EnumerateDirectories(root)
                        .Where(d => File.Exists(Path.Combine(d, "shader_map.tsv")))
                        .Select(d => Path.GetFileName(d).ToUpperInvariant()).ToList();
    }

    /// Documents/gameapp.framework: who built the framework on disk (gameapp_pipeline.py write_framework_stamp).
    private static string FrameworkStampPath => Path.Combine(GameAppMain.Documents, "gameapp.framework");

    // ---- the screen: Games / Keys / Logs under a bottom bar --------------------------------------------------

    private static UIFont Preferred(UIFontTextStyle style, UIFontDescriptorSymbolicTraits traits = 0)
    {
        var font = UIFont.GetPreferredFontForTextStyle(style);
        return traits == 0 ? font : UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(traits), font.PointSize);
    }

    private static UILabel Label(UIFontTextStyle style, UIColor color, int lines = 0, bool bold = false)
    {
        return new UILabel
        {
            Font = Preferred(style, bold ? UIFontDescriptorSymbolicTraits.Bold : 0),
            TextColor = color,
            Lines = lines,
            AdjustsFontForContentSizeCategory = true,
        };
    }

    /// Pin a page's content under the safe area (leading/trailing, never a width taken from the guide: at ViewDidLoad
    /// the guide can still be degenerate, and a negative width silently drops the WHOLE activation batch --
    /// docs/DECISIONS.md, app-net).
    private static void Fill(UIView page, UIView content, nfloat inset)
    {
        content.TranslatesAutoresizingMaskIntoConstraints = false;
        page.AddSubview(content);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            content.TopAnchor.ConstraintEqualTo(page.TopAnchor, 8),
            content.LeadingAnchor.ConstraintEqualTo(page.LeadingAnchor, inset),
            content.TrailingAnchor.ConstraintEqualTo(page.TrailingAnchor, -inset),
            content.BottomAnchor.ConstraintEqualTo(page.BottomAnchor, -8),
        });
    }

    private void BuildKeysPage()
    {
        keysRows.Axis = UILayoutConstraintAxis.Vertical;
        keysRows.Spacing = 10;

        UIButton Pill(string title, string symbol, string id, Action tapped)
        {
            var button = Theme.Pill(title, symbol, false, 48);   // the theme's glass pill, as everywhere else
            button.AccessibilityIdentifier = id;
            button.TouchUpInside += (_, __) => tapped();
            return button;
        }

        // The signing certificate: the ONE secret the device path needs and that cannot be carried from another
        // app (each app has its own Keychain access group). The file is read in place, its bytes go straight into
        // this app's Keychain and its password is typed once -- neither is ever written to disk or logged.
        var card = Theme.Card(22);
        keysRows.TranslatesAutoresizingMaskIntoConstraints = false;
        card.AddSubview(keysRows);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            keysRows.TopAnchor.ConstraintEqualTo(card.TopAnchor, 14),
            keysRows.LeadingAnchor.ConstraintEqualTo(card.LeadingAnchor, 16),
            keysRows.TrailingAnchor.ConstraintEqualTo(card.TrailingAnchor, -16),
            keysRows.BottomAnchor.ConstraintEqualTo(card.BottomAnchor, -14),
        });

        var note = Theme.Label("", Theme.Font(13, UIFontWeight.Regular), Theme.Muted, 0);
        note.Text = "prod.keys opens the game files. The certificate is stored in this app's Keychain, together with its password; "
                  + "nothing is written to a file (unless this install has no Keychain access: then a protected file, unreadable while the phone is locked).";

        var column = new UIStackView(new UIView[]
        {
            SubHeader("Keys and certificate"), card,
            Pill("Import prod.keys", "key", "import-keys", Import),
            Pill("Certificate (.p12)", "checkmark.seal", "import-certificate", ImportCertificate),
            note,
        })
        {
            Axis = UILayoutConstraintAxis.Vertical,
            Spacing = 12,
        };
        // In a scroll view, with full-height rows: a column pinned to both edges squashed the status card empty on an
        // iPhone 15 Pro (2026-11-18); the page scrolls instead.
        card.SetContentCompressionResistancePriority(1000, UILayoutConstraintAxis.Vertical);
        keysRows.SetContentCompressionResistancePriority(1000, UILayoutConstraintAxis.Vertical);
        var scroll = new UIScrollView { AlwaysBounceVertical = true, ShowsVerticalScrollIndicator = true };
        column.TranslatesAutoresizingMaskIntoConstraints = false;
        scroll.AddSubview(column);
        Theme.PinColumn(scroll, column, 0, 16);
        Fill(pages[2], scroll, 16);
    }

    // ---- the NAS (SFTP): exported .ipa files and save copies sent to it (gameapp/GameAppNas.cs) -------------------------

    /// Outside Documents (not in Files): host, port, user, folder, the pinned key. The password is in the Keychain.
    private static string NasConfigPath
    {
        get
        {
            var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Personal), "..", "Library", "Application Support");
            Directory.CreateDirectory(dir);
            return Path.Combine(dir, "gameapp.nas.tsv");
        }
    }
    private static string NasPasswordFile => NasConfigPath.Replace("gameapp.nas.tsv", "gameapp.nas.secret");

    private static NasConfig LoadNas()
    {
        try { return File.Exists(NasConfigPath) ? NasConfig.Parse(File.ReadAllText(NasConfigPath)) : NasConfig.Empty; }
        catch (Exception) { return NasConfig.Empty; }
    }

    private static void SaveNas(NasConfig cfg) => File.WriteAllText(NasConfigPath, cfg.Serialize());

    private static readonly Security.SecRecord NasPasswordQuery = new Security.SecRecord(Security.SecKind.GenericPassword)
    { Service = "dev.nxpocket.nas", Account = "password" };

    /// The NAS password: this app's Keychain, or (an install without Keychain access, docs/DECISIONS.md trap 55) a file
    /// with complete data protection. Never logged.
    private static void StoreNasPassword(string password)
    {
        Security.SecKeyChain.Remove(NasPasswordQuery);
        try { if (File.Exists(NasPasswordFile)) File.Delete(NasPasswordFile); } catch (Exception) { }
        var add = new Security.SecRecord(Security.SecKind.GenericPassword)
        {
            Service = "dev.nxpocket.nas", Account = "password", ValueData = NSData.FromString(password ?? ""),
            Accessible = Security.SecAccessible.WhenUnlockedThisDeviceOnly,
        };
        if (Security.SecKeyChain.Add(add) == Security.SecStatusCode.Success) return;
        File.WriteAllText(NasPasswordFile, password ?? "");
        NSFileManager.DefaultManager.SetAttributes(new NSFileAttributes { ProtectionKey = NSFileProtection.Complete }, NasPasswordFile, out _);
    }

    private static string NasPassword()
    {
        var match = Security.SecKeyChain.QueryAsRecord(NasPasswordQuery, out var code);
        if (code == Security.SecStatusCode.Success && match?.ValueData != null) return match.ValueData.ToString();
        try { return File.Exists(NasPasswordFile) ? File.ReadAllText(NasPasswordFile) : null; } catch (Exception) { return null; }
    }

    private UILabel nasLabel;

    private void RefreshNasLabel()
    {
        if (nasLabel == null) return;
        var cfg = LoadNas();
        nasLabel.Text = cfg.IsComplete ? $"NAS: {cfg.User}@{cfg.Host}:{cfg.Port} › {cfg.Folder}" + (string.IsNullOrEmpty(cfg.HostKey) ? "" : " (key pinned)")
                                       : "NAS: not set";
    }

    private void ConfigureNas()
    {
        var cfg = LoadNas();
        var alert = UIAlertController.Create("NAS (SFTP)", "The server and your account on it. Once connected, you choose the folder (or make a new one).", UIAlertControllerStyle.Alert);
        alert.AddTextField(f => { f.Placeholder = "host (nas.local or 192.168.1.10)"; f.Text = cfg.Host; f.KeyboardType = UIKeyboardType.Url; f.AutocapitalizationType = UITextAutocapitalizationType.None; f.AutocorrectionType = UITextAutocorrectionType.No; });
        alert.AddTextField(f => { f.Placeholder = "port (22)"; f.Text = cfg.Port.ToString(); f.KeyboardType = UIKeyboardType.NumberPad; });
        alert.AddTextField(f => { f.Placeholder = "user name"; f.Text = cfg.User; f.AutocapitalizationType = UITextAutocapitalizationType.None; f.AutocorrectionType = UITextAutocorrectionType.No; });
        alert.AddTextField(f => { f.Placeholder = string.IsNullOrEmpty(NasPassword()) ? "password" : "password (empty: keep the saved one)"; f.SecureTextEntry = true; });
        alert.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
        if (!string.IsNullOrEmpty(cfg.HostKey))
            alert.AddAction(UIAlertAction.Create("Forget the server's key", UIAlertActionStyle.Destructive, _ =>
            { SaveNas(cfg with { HostKey = "" }); Write("NAS: the server's key is forgotten; the next connection pins the one it shows"); RefreshNasLabel(); }));
        alert.AddAction(UIAlertAction.Create("Save", UIAlertActionStyle.Default, _ =>
        {
            var t = alert.TextFields;
            var host = t[0].Text?.Trim() ?? "";
            var next = new NasConfig(host, int.TryParse(t[1].Text, out var port) ? port : 22, t[2].Text?.Trim() ?? "", cfg.Folder,
                                     host == cfg.Host && (int.TryParse(t[1].Text, out var p2) ? p2 : 22) == cfg.Port ? cfg.HostKey : "");
            SaveNas(next);
            if (!string.IsNullOrEmpty(t[3].Text)) StoreNasPassword(t[3].Text);
            Write($"NAS: saved {next.User}@{next.Host}:{next.Port} (the password is in the Keychain, not in this log) — connecting to choose the folder");
            RefreshNasLabel();
            // Connected, the folder is chosen on the server itself (ChooseNasFolder: browse, or make a new one).
            if (next.IsComplete && !string.IsNullOrEmpty(NasPassword())) ChooseNasFolder();
        }));
        PresentWhenFree(alert);
    }

    /// Runs `work` off the main thread with the NAS's settings and password; pins the server key on first use.
    private void WithNas(string what, Func<NasConfig, Renci.SshNet.AuthenticationMethod, string> work, Action done = null)
    {
        var cfg = LoadNas();
        var password = NasPassword();
        if (!cfg.IsComplete || string.IsNullOrEmpty(password))
        {
            var a = UIAlertController.Create("NAS not set", "Set the server, user and password first (Settings › Cloud › NAS › Set up…).", UIAlertControllerStyle.Alert);
            a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
            PresentWhenFree(a);
            return;
        }
        Write($"\n=== NAS: {what}");
        UIApplication.SharedApplication.IdleTimerDisabled = true;
        nint task = UIApplication.SharedApplication.BeginBackgroundTask("nas", null);
        System.Threading.Tasks.Task.Run(() =>
        {
            string error = null, seen = null;
            try { seen = work(cfg, Nas.Password(cfg.User, password)); }
            catch (Exception e) { error = e.Message; }
            BeginInvokeOnMainThread(() =>
            {
                UIApplication.SharedApplication.IdleTimerDisabled = false;
                UIApplication.SharedApplication.EndBackgroundTask(task);
                if (seen != null && string.IsNullOrEmpty(cfg.HostKey)) { SaveNas(cfg with { HostKey = seen }); Write($"NAS: the server's key is pinned ({seen})"); }
                RefreshNasLabel();
                var a = UIAlertController.Create(error == null ? "NAS: done" : "NAS: failed", error ?? what + " — done.", UIAlertControllerStyle.Alert);
                a.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                if (error != null) Write("NAS FAILED: " + error);
                PresentWhenFree(a);
                if (error == null) done?.Invoke();
            });
        });
    }

    private void TestNas() => WithNas("connection test", (cfg, auth) => Nas.Test(cfg, auth, line => BeginInvokeOnMainThread(() => Write(line))));

    /// Sends a file to the NAS (a subfolder of its folder: Exports, or the game's saves), then offers to free the space.
    private void SendToNas(string path, string subfolder, bool offerDelete)
    {
        var size = new FileInfo(path).Length;
        WithNas($"send {Path.GetFileName(path)} ({GameAppUi.SizeText(size)})",
                (cfg, auth) => Nas.Upload(cfg, auth, path, subfolder, line => BeginInvokeOnMainThread(() => Write(line))),
                () =>
                {
                    if (!offerDelete) return;
                    var a = UIAlertController.Create("Free the space?", $"{Path.GetFileName(path)} is on the NAS. Delete it from this {GameAppUi.DeviceName} ({GameAppUi.SizeText(size)})?", UIAlertControllerStyle.Alert);
                    a.AddAction(UIAlertAction.Create("Keep it", UIAlertActionStyle.Cancel, null));
                    a.AddAction(UIAlertAction.Create("Delete", UIAlertActionStyle.Destructive, _ =>
                    {
                        try { File.Delete(path); Write($"deleted from the iPhone: {Path.GetFileName(path)} (it is on the NAS)"); } catch (Exception e) { Write("not deleted: " + e.Message); }
                        RefreshScreen();
                    }));
                    PresentWhenFree(a);
                });
    }

    private static string AppSettingsPath => Path.Combine(GameAppMain.Documents, "gameapp.settings.tsv");
    private static string ShaderCachePath => Path.Combine(GameAppMain.Documents, "shadercache");
    private UILabel cacheSizeLabel;

    private static string ReadAppSettings()
    {
        try { return File.Exists(AppSettingsPath) ? File.ReadAllText(AppSettingsPath) : ""; }
        catch (Exception) { return ""; }
    }

    private static long FolderBytes(string path)
    {
        try
        {
            return Directory.Exists(path)
                ? Directory.EnumerateFiles(path, "*", SearchOption.AllDirectories).Sum(f => new FileInfo(f).Length) : 0;
        }
        catch (Exception) { return 0; }
    }

    private static long ShaderCacheBytes() => FolderBytes(ShaderCachePath);

    private int cacheSizeBusy;

    /// The shader cache is thousands of files: walked off the main thread, the label set when it is known.
    private void RefreshCacheSize()
    {
        if (cacheSizeLabel == null || Interlocked.CompareExchange(ref cacheSizeBusy, 1, 0) != 0) return;
        if (string.IsNullOrEmpty(cacheSizeLabel.Text)) cacheSizeLabel.Text = "Shader cache: …";
        System.Threading.Tasks.Task.Run(() =>
        {
            var bytes = ShaderCacheBytes();
            BeginInvokeOnMainThread(() =>
            {
                if (cacheSizeLabel != null) cacheSizeLabel.Text = "Shader cache: " + GameAppUi.SizeText(bytes);
                Interlocked.Exchange(ref cacheSizeBusy, 0);
            });
        });
    }

    /// The app's build settings (Documents/gameapp.settings.tsv, read by the shim and the pipeline at each build): parallel
    /// shader preparation, and the shader translation cache every game shares (Documents/shadercache).
    private UIView BuildSection()
    {
        var parallel = new UISwitch { On = GameAppUi.ParallelShadersOn(ReadAppSettings()), AccessibilityIdentifier = "setting-parallel-shaders" };
        parallel.ValueChanged += (_, __) =>
        {
            try
            {
                File.WriteAllText(AppSettingsPath, GameAppUi.WithAppSetting(ReadAppSettings(), "parallel_shaders", parallel.On ? "1" : "0"));
                Write($"build setting: parallel shader preparation {(parallel.On ? "on" : "off")} (from the next build)");
            }
            catch (Exception error) { Write($"could not save the build setting: {error.Message}"); }
        };
        var parallelLabel = Label(UIFontTextStyle.Subheadline, UIColor.Label);
        parallelLabel.Text = "Parallel shader preparation";
        var parallelRow = new UIStackView(new UIView[] { parallelLabel, new UIView(), parallel }) { Axis = UILayoutConstraintAxis.Horizontal, Alignment = UIStackViewAlignment.Center };

        var lowPerf = new UISwitch { On = GameAppUi.LowPerformanceOn(ReadAppSettings()), AccessibilityIdentifier = "setting-low-performance" };
        lowPerf.ValueChanged += (_, __) =>
        {
            try
            {
                File.WriteAllText(AppSettingsPath, GameAppUi.WithAppSetting(ReadAppSettings(), "low_performance", lowPerf.On ? "1" : "0"));
                Write($"build setting: low performance {(lowPerf.On ? "on: at most 2 jobs at once" : "off")} (it applies to a build that is running too)");
                if (running) WriteThermalFile();
            }
            catch (Exception error) { Write($"could not save the build setting: {error.Message}"); }
        };
        var lowPerfLabel = Label(UIFontTextStyle.Subheadline, UIColor.Label);
        lowPerfLabel.Text = "Low performance (cooler, slower)";
        var lowPerfRow = new UIStackView(new UIView[] { lowPerfLabel, new UIView(), lowPerf }) { Axis = UILayoutConstraintAxis.Horizontal, Alignment = UIStackViewAlignment.Center };

        cacheSizeLabel = Label(UIFontTextStyle.Subheadline, UIColor.Label);
        RefreshCacheSize();
        var clear = UIButton.FromType(UIButtonType.System);
        clear.SetTitle("Clear", UIControlState.Normal);
        clear.AccessibilityIdentifier = "clear-shader-cache";
        clear.TouchUpInside += (_, __) =>
        {
            var alert = UIAlertController.Create("Clear the shader cache?",
                $"Frees {GameAppUi.SizeText(ShaderCacheBytes())}. Games already built keep their shaders; the next builds translate every shader again.",
                UIAlertControllerStyle.Alert);
            alert.AddAction(UIAlertAction.Create("Clear", UIAlertActionStyle.Destructive, ___ =>
            {
                try { if (Directory.Exists(ShaderCachePath)) Directory.Delete(ShaderCachePath, true); Write("shader cache cleared"); }
                catch (Exception error) { Write($"could not clear the shader cache: {error.Message}"); }
                RefreshCacheSize();
            }));
            alert.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
            PresentViewController(alert, true, null);
        };
        var cacheRow = new UIStackView(new UIView[] { cacheSizeLabel, new UIView(), clear }) { Axis = UILayoutConstraintAxis.Horizontal, Alignment = UIStackViewAlignment.Center };

        return new UIStackView(new UIView[] { parallelRow, lowPerfRow, cacheRow })
        {
            Axis = UILayoutConstraintAxis.Vertical,
            Spacing = 8,
        };
    }

    /// The game's own icon (Documents/games/<id>/icon.jpg: read from its dump in the background, or kept by the
    /// pipeline after an extraction); the last extraction's Documents/extract/icon.jpg for the game the framework was
    /// built from; else null (the card's gamepad).
    private static UIImage CoverOf(string fileName, UiFacts facts)
    {
        try
        {
            var rel = GameAppUi.IconRelativePath(fileName);
            if (rel != null)
            {
                var path = Path.Combine(GameAppMain.Documents, rel.Replace('/', Path.DirectorySeparatorChar));
                if (File.Exists(path)) return CoverFile(fileName, path);
            }
            if (!string.Equals(facts.FrameworkGame, fileName, StringComparison.Ordinal)) return null;
            var extracted = Path.Combine(GameAppMain.Documents, "extract", "icon.jpg");
            return File.Exists(extracted) ? CoverFile(fileName, extracted) : null;
        }
        catch (Exception) { return null; }
    }

    // Decoded covers by game name, valid while the file is the same one with the same modification time (main thread only).
    private static readonly Dictionary<string, (string Path, DateTime Time, UIImage Image)> coverCache = new Dictionary<string, (string, DateTime, UIImage)>();

    private static UIImage CoverFile(string fileName, string path)
    {
        var time = File.GetLastWriteTimeUtc(path);
        if (coverCache.TryGetValue(fileName, out var hit) && hit.Path == path && hit.Time == time) return hit.Image;
        var image = UIImage.FromFile(path);
        coverCache[fileName] = (path, time, image);
        return image;
    }

    private static int iconsRunning;

    /// Reads the icon of every game that has none yet, one dump after another on a background thread, through the
    /// extractor's own functions (dm/tools/nx_extract Program.cs: OpenNsp, Pick, Icon -- only the small control NCA
    /// is read, not the game), then redraws. Needs prod.keys (the control NCA is encrypted). A dump that gives no icon
    /// gets games/<id>/icon.none, tried again only with newer keys.
    private void FetchMissingIcons(UiFacts facts)
    {
        var keys = Path.Combine(GameAppMain.Documents, "prod.keys");
        if (!facts.HasKeys || facts.Running || !File.Exists(keys)) return;
        var keysTime = File.GetLastWriteTimeUtc(keys);
        var todo = new List<(string Dump, string Icon, string None)>();
        foreach (var name in facts.Games ?? new List<string>())
        {
            if (GameAppUi.IsNup(name)) continue;   // the extractor reads NSP/XCI only: a Switch 2 game's icon comes with its build
            var rel = GameAppUi.IconRelativePath(name);
            if (rel == null) continue;
            var icon = Path.Combine(GameAppMain.Documents, rel.Replace('/', Path.DirectorySeparatorChar));
            var none = Path.Combine(GameAppMain.Documents, GameAppUi.NoIconRelativePath(name).Replace('/', Path.DirectorySeparatorChar));
            if (File.Exists(icon)) continue;
            if (File.Exists(none) && File.GetLastWriteTimeUtc(none) >= keysTime) continue;
            todo.Add((Path.Combine(GameAppMain.Documents, name), icon, none));
        }
        if (todo.Count == 0 || Interlocked.CompareExchange(ref iconsRunning, 1, 0) != 0) return;
        System.Threading.Tasks.Task.Run(() =>
        {
            int got = 0;
            try
            {
                foreach (var (dump, icon, none) in todo)
                {
                    byte[] jpeg = null; string why = null;
                    try { jpeg = GameIcons.Read(keys, dump); }
                    catch (Exception error) { why = (error.InnerException ?? error).Message; }
                    try
                    {
                        Directory.CreateDirectory(Path.GetDirectoryName(icon));
                        if (jpeg != null && jpeg.Length > 0) { File.WriteAllBytes(icon, jpeg); got++; }
                        else File.WriteAllText(none, (why ?? "no icon in the control data") + "\n");
                    }
                    catch (Exception) { }
                    Console.Error.WriteLine($"[gameapp] icon of {Path.GetFileName(dump)}: {(jpeg != null ? jpeg.Length + " bytes" : why ?? "none")}");
                }
            }
            finally
            {
                Interlocked.Exchange(ref iconsRunning, 0);
                GC.Collect();   // the dumps' file handles the NCAs held
            }
            if (got > 0) InvokeOnMainThread(RefreshScreen);
        });
    }

    /// Redraw the whole screen from the facts on disk: the banner, the carousel and list, the selected game, the Keys rows. Called at
    /// launch, when the app comes back to the foreground, when a run starts and ends, after an import or a
    /// certificate, and when a game is selected.
    private void RefreshScreen()
    {
        UiFacts facts;
        try { facts = GatherFacts(); }
        catch (Exception error) { Write($"screen: the facts could not be read: {error.GetType().Name}: {error.Message}"); return; }
        // The Keys page first, and each part on its own: a part that throws (a game's file, an icon, the storage
        // sizes) is logged and the rest is still drawn (the Keys card came up empty on a phone, 2026-11-18).
        sharedSegments = null; sharingSegments = true;   // the Library's and Settings' storage bars walk the folders once
        try
        {
            Guarded("keys", () => FillKeysRows(facts));
            Guarded("settings", () => RefreshSettingsPage(facts));
            Guarded("storage", RefreshCacheSize);
            Guarded("icons", () => FetchMissingIcons(facts));
            if (firstFrameDone) Guarded("cloud icons", FetchCloudIcons);
            var games = facts.Games ?? new List<string>();
            Guarded("selection", () => { if (!IsCloudOnly(selected)) selected = GameAppUi.SelectGame(games, selected, facts); });
            Guarded("games", () => RefreshGamesPage(facts, games));
            Guarded("player", () => RefreshPlayer(facts));
            Guarded("pointer", () => Theme.Hover(View));
        }
        finally { sharingSegments = false; sharedSegments = null; }
    }

    private List<GameAppUi.StorageSegment> sharedSegments;
    private bool sharingSegments;

    /// The storage bar's segments (dumps, built game, shader libraries, exported .ipa, free space). Within one
    /// RefreshScreen the folders are measured once and both bars share the result.
    private List<GameAppUi.StorageSegment> CurrentStorageSegments()
    {
        if (sharingSegments && sharedSegments != null) return sharedSegments;
        var docs = GameAppMain.Documents;
        var segments = GameAppUi.StorageSegments(DumpBytes(),
            FolderBytes(Path.Combine(docs, "extract")) + FolderBytes(Path.Combine(docs, "DmGameProbe.framework")),
            FolderBytes(Path.Combine(docs, "shaders")) + ShaderCacheBytes(),
            FolderBytes(Path.Combine(docs, "Exports")), FreeBytes());
        if (sharingSegments) sharedSegments = segments;
        return segments;
    }

    private int refreshFailures;

    private void Guarded(string part, Action action)
    {
        try { action(); }
        catch (Exception error)
        {
            if (refreshFailures++ < 20)
                Write($"screen: the {part} part could not be drawn: {error.GetType().Name}: {error.Message}\n{error.StackTrace?.Split('\n').FirstOrDefault()?.Trim()}");
        }
    }

    private void FillKeysRows(UiFacts facts)
    {
        // The Keys page: the rows about the device, the keys and the two entitlements, not about a game.
        foreach (var row in keysRows.ArrangedSubviews.ToArray()) { keysRows.RemoveArrangedSubview(row); row.RemoveFromSuperview(); }
        foreach (var row in GameAppUi.Checklist(facts))
            if (row.Label == "prod.keys" || row.Label == "Signing certificate" || row.Label == "Memory ceiling" || row.Label == "Address space"
                || row.Label == "RAM unlocked" || row.Label == "Extended addressing" || row.Label == "Play inside iBuilder")
                keysRows.AddArrangedSubview(ChecklistLabel(row));
    }

    /// One checklist row: a coloured mark (✓ green, ! orange, ✗ red, · neutral), the label in bold, the detail in
    /// the secondary colour. System colours and the Footnote text style, so dark mode and Dynamic Type just work.
    private static UILabel ChecklistLabel(ChecklistRow row)
    {
        var (mark, color, word) = row.Level switch
        {
            CheckLevel.Ok => ("✓", UIColor.SystemGreen, "ok"),
            CheckLevel.Warn => ("!", UIColor.SystemOrange, "warning"),
            CheckLevel.Missing => ("✗", UIColor.SystemRed, "missing"),
            _ => ("·", UIColor.SecondaryLabel, "info"),
        };
        var font = UIFont.GetPreferredFontForTextStyle(UIFontTextStyle.Footnote);
        var bold = UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(UIFontDescriptorSymbolicTraits.Bold),
                                         font.PointSize);
        var text = new NSMutableAttributedString();
        text.Append(new NSAttributedString(mark + " ", new UIStringAttributes { Font = bold, ForegroundColor = color }));
        text.Append(new NSAttributedString(row.Label + "  ", new UIStringAttributes { Font = bold, ForegroundColor = UIColor.Label }));
        text.Append(new NSAttributedString(row.Detail, new UIStringAttributes { Font = font, ForegroundColor = UIColor.SecondaryLabel }));
        return new UILabel
        {
            Lines = 0,
            AttributedText = text,
            AdjustsFontForContentSizeCategory = true,
            AccessibilityLabel = $"{row.Label}: {row.Detail}, {word}",
        };
    }

    /// The primary button: what it does is decided from the facts at the moment of the tap, not from what it said when
    /// it was drawn (a file may have arrived since).
    private void PrimaryTapped()
    {
        if (IsCloudOnly(selected)) { DownloadSelected(); return; }   // a game only in the cloud: the button downloads it
        var facts = GatherFacts();
        selected = GameAppUi.SelectGame(facts.Games, selected, facts);
        if (selected == null) { Import(); return; }
        var next = GameAppUi.GameAction(selected, facts);
        // A hot phone is asked first; "Start anyway" and a cooled phone come back here.
        if ((next.Kind == ActionKind.Boot || next.Kind == ActionKind.Run) && AskIfHot(PrimaryTapped)) return;
        switch (next.Kind)
        {
            case ActionKind.Import: Import(); break;
            case ActionKind.Boot:
                if (builtThisProcess) { AskToRestartForGame(selected); break; }
                AutoCopySaves(selected);
                Write("no rebuild: this tap boots the framework this version of the app already built and signed");
                BeginRun("boot-only (the framework is built and signed)", null, skipExtract: true, bootOnly: true);
                break;
            case ActionKind.Run: SetUpAndPlay(selected); break;
            // A distribution certificate: the built game runs only from an app's own bundle (GameAppUi.CannotRunHere).
            case ActionKind.Export: ConfirmExport(selected); break;
            case ActionKind.Busy: break;   // the button is disabled while a run goes; nothing to do
        }
        RefreshScreen();
    }

    /// The whole chain for THIS game file, with no picker. A framework of another game may be on disk, and a pending
    /// boot request would make BeginRunWithArguments boot it instead: the request is withdrawn first (rebuildRequested),
    /// so the game that was asked for is the one that is built.
    private void SetUpAndPlay(string game)
    {
        if (running) { Write("a build is already running"); return; }
        var keys = Path.Combine(GameAppMain.Documents, "prod.keys");
        bool nup = GameAppUi.IsNup(game);   // a Switch 2 package is not encrypted: no keys
        if (!nup && !File.Exists(keys)) { Write("no prod.keys in Documents: the tool cannot read an NSP without them"); return; }
        var path = Path.Combine(GameAppMain.Documents, game);
        if (!File.Exists(path)) { Write($"{game} is no longer in Documents"); return; }
        rebuildRequested = true;
        BeginRunWithArguments(keys, path, "--romfs-image");
    }

    // ---- the game's settings (the gear) ----------------------------------------------------------------------

    private static string SettingsPath(string game)
    {
        var relative = GameAppUi.SettingsRelativePath(game);
        return relative == null ? null : Path.Combine(GameAppMain.Documents, relative.Replace('/', Path.DirectorySeparatorChar));
    }

    private GameAppUi.GameSettings LoadSettings(string game)
    {
        try
        {
            var path = SettingsPath(game);
            return path != null && File.Exists(path) ? GameAppUi.GameSettings.Parse(File.ReadAllText(path)) : GameAppUi.GameSettings.Defaults;
        }
        catch (Exception) { return GameAppUi.GameSettings.Defaults; }
    }

    private void SaveSettings(string game, GameAppUi.GameSettings settings)
    {
        var path = SettingsPath(game);
        if (path == null) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            File.WriteAllText(path, GameAppUi.GameSettings.Serialize(settings));
            Write($"settings saved: {GameAppUi.SettingsRelativePath(game)} ({settings.Mode}, fps {settings.Fps}, {settings.Picture}, show fps {(settings.ShowFps ? 1 : 0)})");
        }
        catch (Exception error) { Write($"could not save {path}: {error.GetType().Name}: {error.Message}"); }
    }

    private static string LauncherPath(string game)
    {
        var relative = GameAppUi.LauncherRelativePath(game);
        return relative == null ? null : Path.Combine(GameAppMain.Documents, relative.Replace('/', Path.DirectorySeparatorChar));
    }

    /// The engine launcher's settings list the pipeline wrote after the game's build, filtered for this screen; null when
    /// there is none (the game was never built) or it has no usable row.
    private static IReadOnlyList<GameAppUi.LauncherRow> LoadLauncher(string game)
    {
        try
        {
            var path = LauncherPath(game);
            if (path == null || !File.Exists(path)) return null;
            var rows = GameAppUi.ParseLauncher(File.ReadAllText(path));
            return rows.Count == 0 ? null : GameAppUi.FilterForScreen(rows, (int) UIScreen.MainScreen.MaximumFramesPerSecond);
        }
        catch (Exception) { return null; }
    }

    private static Dictionary<string, string> LoadSettingValues(string game)
    {
        try
        {
            var path = SettingsPath(game);
            return path != null && File.Exists(path) ? GameAppUi.ParseSettingValues(File.ReadAllText(path)) : new Dictionary<string, string>();
        }
        catch (Exception) { return new Dictionary<string, string>(); }
    }

    private void SaveSettingValues(string game, IReadOnlyList<GameAppUi.LauncherRow> rows, Dictionary<string, string> values)
    {
        var path = SettingsPath(game);
        if (path == null) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            var text = GameAppUi.SerializeSettingValues(values, rows);
            File.WriteAllText(path, text);
            Write($"settings saved: {GameAppUi.SettingsRelativePath(game)} ({text.Split('\n', StringSplitOptions.RemoveEmptyEntries).Length} non-default)");
        }
        catch (Exception error) { Write($"could not save {path}: {error.GetType().Name}: {error.Message}"); }
    }

    /// profile.tsv's title_id: the game the extraction and the framework on disk were built for, or null.
    private static string BuiltTitleId()
    {
        try
        {
            var path = Path.Combine(GameAppMain.Documents, "profile.tsv");
            if (!File.Exists(path)) return null;
            foreach (var line in File.ReadLines(path))
                if (line.StartsWith("title_id\t", StringComparison.Ordinal)) return line.Substring(9).Trim().ToUpperInvariant();
        }
        catch (Exception) { }
        return null;
    }

    /// Bytes the plan's paths hold now (missing ones count 0).
    private static long PlanBytes(GameAppUi.DeletionPlan plan)
    {
        long total = 0;
        foreach (var rel in plan.Files)
        {
            try { var f = new FileInfo(Path.Combine(GameAppMain.Documents, rel)); if (f.Exists) total += f.Length; } catch (Exception) { }
        }
        foreach (var rel in plan.Folders)
        {
            try
            {
                var d = Path.Combine(GameAppMain.Documents, rel);
                if (Directory.Exists(d)) total += Directory.EnumerateFiles(d, "*", SearchOption.AllDirectories).Sum(f => new FileInfo(f).Length);
            }
            catch (Exception) { }
        }
        return total;
    }

    private bool exporting;

    /// "Export as .ipa…": the built game as an app of its own (GameAppExport.cs). Asks first (size, space, what an
    /// installer does with it), then writes Documents/Exports/<Name>.ipa off the main thread and offers to share it.
    private void ConfirmExport(string game)
    {
        if (running || exporting || game == null) return;
        var tid = BuiltTitleId();
        if (tid == null) { Write("export: the built game has no title id (profile.tsv): build it again"); return; }
        long size = 0;
        foreach (var rel in new[] { "DmGameProbe.framework", "extract/flat", "shaders/" + tid })
            size += FolderBytes(Path.Combine(GameAppMain.Documents, rel));
        long romfs = 0;
        try { romfs = new FileInfo(Path.Combine(GameAppMain.Documents, "extract", "RomFS.img")).Length; } catch (Exception) { }
        size += FolderBytes(NSBundle.MainBundle.BundlePath);
        var name = GameAppUi.DisplayName(game);
        var romfsName = GameAppUi.RomfsFileName(name, tid);
        // Light for big games only: an installer repacks the whole .ipa, and a 15.5 GB one was more than FlareStore could do.
        bool lightFirst = GameAppUi.LightIpaPreferred(romfs);
        var folder = GameAppUi.ExportFileName(name);
        var alert = UIAlertController.Create("Export as .ipa",
            $"{name} becomes an app of its own, in Files › iBuilder › Exports, to install with SideStore or FlareStore.\n\n"
            + $"Light IPA, {GameAppUi.SizeText(size)}: the game files ({GameAppUi.SizeText(romfs)}) stay beside it, in Exports › {folder} as \"{romfsName}\" "
            + "(no extra space). The first time it opens, the app asks for that file once and reads the game from it: keep it, and keep iBuilder.\n\n"
            + $"Full IPA, {GameAppUi.SizeText(size + romfs)}: everything inside, an app that needs nothing else"
            + (lightFirst ? ". Warning: an installer may not manage to repack an .ipa this large." : ".")
            + "\n\nWith a free Apple ID an app takes one of your three slots.",
            UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
        var fullAction = UIAlertAction.Create($"Full IPA ({GameAppUi.SizeText(size + romfs)})", UIAlertActionStyle.Default, _ => StartExport(game, tid, slim: false));
        var slimAction = UIAlertAction.Create($"Light IPA ({GameAppUi.SizeText(size)})", UIAlertActionStyle.Default, _ => StartExport(game, tid, slim: true));
        alert.AddAction(fullAction);
        alert.AddAction(slimAction);
        alert.PreferredAction = lightFirst ? slimAction : fullAction;
        PresentWhenFree(alert);
    }

    private void StartExport(string game, string tid, bool slim)
    {
        exporting = true;
        ShowTab(3);   // the log follows the export
        Write($"\n=== export {GameAppUi.DisplayName(game)} as a {(slim ? "light" : "full")} .ipa");
        UIApplication.SharedApplication.IdleTimerDisabled = true;
        nint task = UIApplication.SharedApplication.BeginBackgroundTask("export", null);
        var documents = GameAppMain.Documents;
        var bundle = NSBundle.MainBundle.BundlePath;
        System.Threading.Tasks.Task.Run(() =>
        {
            string path = null, failure = null;
            var started = DateTime.Now;
            try { path = GameExport.Export(documents, bundle, game, tid, slim, line => BeginInvokeOnMainThread(() => Write(line))); }
            catch (Exception error) { failure = error.Message; }
            BeginInvokeOnMainThread(() =>
            {
                exporting = false;
                UIApplication.SharedApplication.IdleTimerDisabled = false;
                UIApplication.SharedApplication.EndBackgroundTask(task);
                RefreshScreen();
                if (path == null)
                {
                    Write($"export FAILED: {failure}");
                    var fail = UIAlertController.Create("Export failed", failure, UIAlertControllerStyle.Alert);
                    fail.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
                    PresentWhenFree(fail);
                    return;
                }
                Write($"export: {Took(DateTime.Now - started)}; install it from Files › iBuilder › Exports, or share it now");
                var done = UIAlertController.Create("Export done", slim
                    ? $"{Path.GetFileName(path)} is in Files › iBuilder › Exports (also in Settings › Exported apps). Share it to SideStore "
                      + $"or FlareStore to install it. The first time it opens, show it \"{GameAppUi.RomfsFileName(GameAppUi.DisplayName(game), tid)}\", "
                      + $"in Exports › {GameAppUi.ExportFileName(GameAppUi.DisplayName(game))}: keep that file, the app reads the game from it."
                    : $"{Path.GetFileName(path)} is in Files › iBuilder › Exports. Share it to SideStore or FlareStore to install it. "
                      + "Delete it from Files once installed: it takes as much space as the game.",
                    UIAlertControllerStyle.Alert);
                done.AddAction(UIAlertAction.Create("Later", UIAlertActionStyle.Cancel, null));
                if (LoadNas().IsComplete)
                    done.AddAction(UIAlertAction.Create("Send to the NAS", UIAlertActionStyle.Default, __ => SendToNas(path, "Exports", offerDelete: true)));
                done.AddAction(UIAlertAction.Create("Share…", UIAlertActionStyle.Default, __ =>
                {
                    var share = new UIActivityViewController(new NSObject[] { NSUrl.FromFilename(path) }, null);
                    Theme.Anchor(share, null, View);
                    PresentWhenFree(share);
                }));
                PresentWhenFree(done);
            });
        });
    }

    /// "Delete this game…": what goes and the space it frees, with or without the saves; nothing while a run is going.
    private void ConfirmDelete(string game)
    {
        if (running || game == null) return;
        var facts = GatherFacts();
        var all = Dumps().Select(Path.GetFileName).ToList();
        var builtId = BuiltTitleId();
        var keep = GameAppUi.PlanDeletion(game, all, facts.FrameworkGame, builtId, keepSaves: true);
        var everything = GameAppUi.PlanDeletion(game, all, facts.FrameworkGame, builtId, keepSaves: false);
        long keepBytes = PlanBytes(keep), allBytes = PlanBytes(everything);
        var extra = keep.Files.Where(f => f != game && all.Contains(f)).ToList();
        var message = $"Frees {GameAppUi.SizeText(keepBytes)}: the game file, its shaders" + (keep.BuildToo ? ", its build (the extracted game)" : "")
                      + " and its settings."
                      + (extra.Count > 0 ? $"\nAlso its update and add-ons: {string.Join(", ", extra.Select(GameAppUi.DisplayName))}." : "")
                      + "\nSaves are kept unless you choose otherwise. This cannot be undone.";
        var alert = UIAlertController.Create($"Delete {GameAppUi.DisplayName(game)}?", message, UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("Delete, keep saves", UIAlertActionStyle.Destructive, _ => PerformDelete(game, keep)));
        alert.AddAction(UIAlertAction.Create($"Delete with saves ({GameAppUi.SizeText(allBytes)})", UIAlertActionStyle.Destructive, _ => PerformDelete(game, everything)));
        alert.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, null));
        PresentViewController(alert, true, null);
    }

    private void PerformDelete(string game, GameAppUi.DeletionPlan plan)
    {
        if (running) return;
        long freed = PlanBytes(plan);
        int removed = 0, failed = 0;
        foreach (var rel in plan.Files)
        {
            var path = Path.Combine(GameAppMain.Documents, rel);
            try { if (File.Exists(path)) { File.Delete(path); removed++; } }
            catch (Exception error) { failed++; Write($"delete: {rel} not removed ({error.Message})"); }
        }
        foreach (var rel in plan.Folders)
        {
            var path = Path.Combine(GameAppMain.Documents, rel);
            try { if (Directory.Exists(path)) { Directory.Delete(path, true); removed++; } }
            catch (Exception error) { failed++; Write($"delete: {rel}/ not removed ({error.Message})"); }
        }
        Write($"deleted {GameAppUi.DisplayName(game)}: {removed} item(s), {GameAppUi.SizeText(freed)} freed" + (failed > 0 ? $", {failed} could not be removed" : ""));
        if (selected == game) selected = null;
        RefreshScreen();
    }

    /// The game's settings, as the sheet draws them: the engine launcher's rows when the game was built, else the four
    /// fixed ones. Used by the sheet and by an exported game's one screen (embedded).
    private GameAppSettingsViewController SettingsFor(string game, bool canRebuild)
    {
        var sheet = new GameAppSettingsViewController(GameAppUi.DisplayName(game), GameAppUi.SettingsRelativePath(game) != null,
                                                      LoadSettings(game), s => SaveSettings(game, s),
                                                      canRebuild ? () => SetUpAndPlay(game) : null);
        var launcher = LoadLauncher(game);
        if (launcher != null)
            sheet.UseLauncher(launcher, LoadSettingValues(game), v => SaveSettingValues(game, launcher, v));
        return sheet;
    }

    // ---- a game's saves (GameAppSaves.cs, GameAppSavesUi.cs) ---------------------------------------------------------------

    /// "Mods & cheats": the game's mods (GameAppMods.cs, GameAppModsUi.cs). `built`: a build is on this iPhone, so a change to the code's patches needs a rebuild.
    private void ShowMods(string game, bool built)
    {
        var tid = GameAppUi.BaseTitleIdOf(game);
        if (tid == null) { Write("mods: this file name carries no [title id]"); return; }
        var sheet = new GameAppModsViewController(GameAppMain.Documents, tid, GameAppUi.DisplayName(game), built, text => Write(text));
        Theme.StyleSheet(sheet);
        PresentWhenFree(sheet);
    }

    private void ShowSaves(string game)
    {
        var tid = GameAppUi.BaseTitleIdOf(game);
        if (tid == null) { Write("saves: this file name carries no [title id]"); return; }
        var sheet = new GameAppSavesViewController(GameAppMain.Documents, tid, GameAppUi.DisplayName(game), text => Write(text));
        if (!GameExport.IsPlayer && LoadNas().IsComplete)
        {
            var folder = "Saves/" + GameAppUi.ExportFileName(GameAppUi.DisplayName(game)) + " [" + tid + "]";
            sheet.SendToNas = copy => DismissViewController(true, () => SendToNas(copy, folder, offerDelete: false));
        }
        Theme.StyleSheet(sheet);
        PresentWhenFree(sheet);
    }

    /// A copy of the save before every launch: the ten newest are kept (GameSaves.KeepAuto). A game that ruins its save
    /// (a crash mid-write, a wrong choice) can always go back to how it was when it last started.
    private void AutoCopySaves(string game)
    {
        var tid = GameAppUi.BaseTitleIdOf(game);
        if (tid == null) return;
        try
        {
            var p = GameSaves.Create(GameAppMain.Documents, tid, GameAppUi.DisplayName(game), "auto", DateTime.Now);
            if (p != null) Write($"saves: copy before the launch, {Path.GetFileName(p)}");
        }
        catch (Exception error) { Write($"saves: no copy before the launch ({error.Message}) — the game starts anyway"); }
    }

    // ---- an exported game's app: one screen, the game and its settings (GameAppExport.cs) ------------------------------

    private UIButton playerPlay;
    private UILabel playerStatus;

    /// No tabs: no keys, no certificate, no import, no build, no logs page. The game's icon and name, Play, and its
    /// settings below, so it reads as the game's own app and not as iBuilder. The pages iBuilder builds still exist
    /// (the screen's refresh and the run's log write to them); they are just never shown.
    private void ShowSettings()
    {
        if (selected == null || running) return;
        var game = selected;
        var facts = GatherFacts();
        // "Rebuild from scratch" is the way out of a boot: offered for the game whose framework is on disk, when a
        // rebuild could actually start (GameAppUi.ShowRebuild).
        var ready = GameAppUi.GameStatusOf(game, facts) == GameAppUi.GameStatus.Ready;
        var canRebuild = !GameExport.IsPlayer && GameAppUi.ShowRebuild(facts) && ready;
        var sheet = SettingsFor(game, canRebuild);
        if (GameAppUi.BaseTitleIdOf(game) != null) sheet.ManageSaves = () => ShowSaves(game);
        if (!GameExport.IsPlayer && GameAppUi.BaseTitleIdOf(game) != null) sheet.ManageMods = () => ShowMods(game, ready);
        if (!GameExport.IsPlayer && GameAppUi.ContentKindOf(game) == GameAppUi.ContentKind.Base)
        {
            var names = Dumps().Select(Path.GetFileName).ToList();
            var update = GameAppUi.UpdateFor(game, names);
            var dlcs = GameAppUi.DlcFor(game, names);
            var lines = new List<string>
            {
                update != null ? $"Update: v{GameAppUi.VersionOf(update)} ({update})" : "Update: none (the game runs as dumped)",
                dlcs.Count == 0 ? "DLC: none" : $"DLC: {dlcs.Count}" + string.Concat(dlcs.Select(d => "\n  • " + GameAppUi.DisplayName(d))),
                GameAppUi.IsNup(game) ? "A .nup carries its own update and DLC: they are read from it."
                    : "Import the update or DLC files (.nsp, .nsz, .xci) like a game: they attach to it by their title id. The newest update and every DLC go into the next build"
                      + (GameAppUi.ContentChangedSinceBuild(game, names, facts) ? " — something changed since the last build: rebuild to include it." : "."),
            };
            sheet.ContentInfo = string.Join("\n", lines);
            sheet.AddContent = Import;
        }
        if (!GameExport.IsPlayer)
        {
            sheet.DeleteGame = () => ConfirmDelete(game);
            // Export: only the game whose framework is on disk, built by this app (its build is what goes in the .ipa).
            if (ready) sheet.ExportGame = () => ConfirmExport(game);
        }
        Theme.StyleSheet(sheet);
        PresentViewController(sheet, true, null);
    }

    private void BeginRunWithArguments(string keys, string game, string mode)
    {
        // A pending boot-only request outranks a tap. The framework on disk was built and SIGNED by the run that
        // wrote that request, so re-running the chain here would spend ~2.5 minutes to end by refusing the boot
        // and writing the very same request -- arriving where we already are. A tap is a request to run, and the
        // run that is owed is the boot -- unless the user asked for the rebuild (the rebuild button): a request can
        // outlive what it was for (a framework built by an older copy of the app), and it is withdrawn here, with a
        // game chosen, so that cancelling the sheet loses nothing.
        if (rebuildRequested)
        {
            rebuildRequested = false;
            try
            {
                if (File.Exists(BootOnlyPath)) File.Delete(BootOnlyPath);
                Write("rebuild requested: Documents/gameapp.boot-only withdrawn — the whole chain runs");
            }
            catch (Exception error)
            {
                Write($"could not withdraw {BootOnlyPath}: {error.GetType().Name}: {error.Message} — not starting");
                return;
            }
        }
        else if (GameAppUi.ChooseAction(GatherFacts()).Kind == ActionKind.Boot)
        {
            // The same decision the main button took (GameAppUi.ChooseAction): a framework THIS build made is on disk.
            // A request alone is not enough -- it may point at a framework an older build made, or at none.
            Write("no rebuild: a framework this version of the app built and signed is on disk — this tap boots it");
            BeginRun("boot-only (the framework is built and signed)", null, skipExtract: true, bootOnly: true);
            return;
        }
        var output = Path.Combine(GameAppMain.Documents, "extract");
        // The newest update for this base game, when one is in Documents: the extractor merges it (its own update argument).
        var updateName = GameAppUi.UpdateFor(Path.GetFileName(game), Dumps().Select(Path.GetFileName));
        var update = updateName != null ? Path.Combine(GameAppMain.Documents, updateName) : "-";
        if (updateName != null) Write($"update: {updateName} is merged over the base game");
        var arguments = new[] { keys, game, update, output, mode };
        // Store NAMES, never absolute paths. A container path is only valid for the install that wrote it, and an
        // app reinstall gives the container a new UUID, so a stored absolute path survives as a dead reference
        // (measured: DirectoryNotFoundException on /…/Documents/prod.keys with the old UUID). The app re-anchors
        // each name under the CURRENT Documents at launch (AutoRunFromArgs / ResolveUnderDocuments).
        var portable = new[]
        {
            RelativeToDocuments(keys), RelativeToDocuments(game), update == "-" ? "-" : RelativeToDocuments(update), RelativeToDocuments(output), mode,
        };
        File.WriteAllLines(Path.Combine(GameAppMain.Documents, "gameapp.args"), portable);
        BeginRun(Path.GetFileName(game), arguments, skipExtract: false);
    }

    /// A build ran in this process (trap 49): the game then needs a fresh launch, so Play closes the app first.
    private static bool builtThisProcess;

    [DllImport("libc")] private static extern int getpid();
    [DllImport("libc")] private static extern int kill(int pid, int sig);

    /// Play right after a build: the guest stalled in the process that built (logs 12), and iOS cannot relaunch an app,
    /// so the player is asked to reopen it; the reopened launch starts the game (Documents/gameapp.boot-only, armed).
    private void AskToRestartForGame(string game)
    {
        var alert = UIAlertController.Create($"{GameAppUi.DisplayName(game)} is ready",
            "The build used a lot of this app's memory. GameApp closes so the game starts with all of it: open GameApp again and the game starts.",
            UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("Close and play", UIAlertActionStyle.Default, _ =>
        {
            try
            {
                File.WriteAllText(BootOnlyPath, "boot " + Path.Combine(GameAppMain.Documents, "DmGameProbe.framework", "DmGameProbe")
                                                + " in a fresh process (the player tapped Play after a build)\n");
                Write("Play after a build: Documents/gameapp.boot-only written; the app closes and the next launch starts the game");
            }
            catch (Exception error) { Write($"could not write {BootOnlyPath}: {error.Message}"); return; }
            kill(getpid(), 15);   // SIGTERM: the app's own clean end, as Quit game uses
        }));
        alert.AddAction(UIAlertAction.Create("Not now", UIAlertActionStyle.Cancel, null));
        PresentViewController(alert, true, null);
    }

    /// The embedded Python ran in this process (gameapp_run_chain came back). It is never started a second time here:
    /// until 2026-11-18 every run ended the app (a SystemExit in Py_BytesMain), so a second Py_BytesMain after a
    /// finalized interpreter has never been exercised, and the free-threaded runtime's statically registered modules
    /// are not known to survive it. A second run is done by a fresh launch instead (AskToRestartForRun).
    private static bool pythonRanThisProcess;

    /// A run asked for after one already came back in this process: the app closes and the next launch does it. A build
    /// resumes from Documents/gameapp.args (the caller wrote it, the launch path reads it); a boot from an armed
    /// Documents/gameapp.boot-only.
    private void AskToRestartForRun(string name, bool bootOnly)
    {
        var alert = UIAlertController.Create("GameApp starts again",
            bootOnly ? "A run already happened in this session. GameApp closes; open it again and the game starts."
                     : "A run already happened in this session. GameApp closes; open it again and the build starts by itself.",
            UIAlertControllerStyle.Alert);
        alert.AddAction(UIAlertAction.Create("Close", UIAlertActionStyle.Default, _ =>
        {
            try
            {
                if (bootOnly)
                    File.WriteAllText(BootOnlyPath, "boot " + Path.Combine(GameAppMain.Documents, "DmGameProbe.framework", "DmGameProbe")
                                                    + " in a fresh process (a run already came back in the last one)\n");
                Write($"{name}: a run already came back in this process — the app closes and the next launch does it");
            }
            catch (Exception error) { Write($"could not record the request: {error.Message}"); return; }
            kill(getpid(), 15);
        }));
        alert.AddAction(UIAlertAction.Create("Not now", UIAlertActionStyle.Cancel, _ =>
        {
            // Not now: the request is not left to start by itself at the next launch.
            if (!bootOnly) ConsumeArgs(Path.Combine(GameAppMain.Documents, "gameapp.args"));
        }));
        PresentWhenFree(alert);
    }

    /// The game's DLC (its add-ons in Documents, GameAppUi.DlcFor), after the game itself: each one's RomFS as
    /// Documents/games/<id>/aoc/<index>.img (nx_extract --aoc), the folder the boot names as DM_AOC_DIR. The index is
    /// the DLC's title id minus the game's AddOnContentBaseId (its NACP), as the engine's NSP script computes it. Images
    /// of DLC no longer in Documents go first. A DLC that cannot be read is logged and left out: the game still builds.
    /// Runs on the run's worker thread.
    private void ExtractDlc(string gameName, string[] arguments)
    {
        void Say(string line) => BeginInvokeOnMainThread(() => Write(line));
        var tid = GameAppUi.BaseTitleIdOf(gameName);
        if (tid == null || arguments == null || arguments.Length < 4) return;
        var names = Dumps().Select(Path.GetFileName).ToList();
        var dlcs = GameAppUi.DlcFor(gameName, names);
        var aoc = Path.Combine(GameAppMain.Documents, "games", tid, "aoc");
        try
        {
            if (Directory.Exists(aoc)) foreach (var old in Directory.GetFiles(aoc, "*.img")) File.Delete(old);
        }
        catch (Exception error) { Say($"DLC: could not clear {aoc}: {error.Message}"); }
        if (dlcs.Count == 0) return;
        Directory.CreateDirectory(aoc);
        ulong baseId = ulong.Parse(tid, System.Globalization.NumberStyles.HexNumber), aocBase = 0;
        try
        {
            var nacp = File.ReadAllBytes(Path.Combine(arguments[3], "control.nacp"));
            if (nacp.Length >= 0x3078) aocBase = BitConverter.ToUInt64(nacp, 0x3070);   // AddOnContentBaseId
        }
        catch (Exception) { }
        Say($"DLC: {dlcs.Count} for this game, add-on base {(aocBase != 0 ? aocBase.ToString("X16") : (baseId + 0x1000).ToString("X16") + " (base + 0x1000)")}");
        foreach (var dlc in dlcs)
        {
            var index = GameAppUi.AocIndex(dlc, baseId, aocBase);
            if (index == null) { Say($"DLC: {dlc}: no index for its title id — left out"); continue; }
            var source = Path.Combine(GameAppMain.Documents, dlc);
            string converted = null;
            if (dlc.EndsWith(".nsz", StringComparison.OrdinalIgnoreCase))
            {
                converted = GameAppMain.NszToNsp(source, out var why);
                if (converted == null) { Say($"DLC: {dlc}: the .nsz could not be turned back into an NSP ({why}) — left out"); continue; }
                source = converted;
            }
            int rc;
            try
            {
                rc = P.Main(new[] { arguments[0], "--aoc", source, Path.Combine(aoc, index + ".img"), GameAppUi.TitleIdHexOf(dlc), "--romfs-image" });
            }
            catch (Exception error) { rc = -1; Say($"DLC: {dlc}: {error.GetType().Name}: {error.Message}"); }
            finally { if (converted != null) try { File.Delete(converted); } catch (Exception) { } }
            Say(rc == 0 ? $"DLC: {dlc} -> aoc/{index}.img" : $"DLC: {dlc} could not be read (exit {rc}) — left out, the game still builds");
        }
    }

    private void BeginRun(string name, string[] arguments, bool skipExtract, bool bootOnly = false)
    {
        if (running) return;
        // A Switch 2 game (.nup): no extractor run here (it reads NSP/XCI only); the chain reads the package itself
        // (gameapp_pipeline.extract_nup, from the config's nup= line), with no keys.
        if (arguments != null && arguments.Length > 1 && GameAppUi.IsNup(arguments[1])) skipExtract = true;
        if (pythonRanThisProcess) { AskToRestartForRun(name, bootOnly); return; }
        // Pre-flight: a stale or missing keys file must be a clear refusal, not an exception deep inside the
        // C# extraction tool that the app then reports as "ok" (measured on the device). The path has already
        // been re-anchored under the current Documents by the caller.
        if (!skipExtract && arguments != null && arguments.Length >= 1 && !File.Exists(arguments[0]))
        {
            Write($"no keys at {arguments[0]} — put prod.keys in Documents (via Import) first; not starting");
            return;
        }
        // Pre-flight on the signing identity too. It does not stop the run -- the rewrite is worth doing on its
        // own -- but a missing certificate has to be named HERE rather than 2.5 minutes later as an
        // unintelligible rc=12 at the end of the chain (measured on the device). A boot-only run signs nothing,
        // so the warning does not apply to it: telling the user the chain "will fail at stage 4b" seconds before
        // it boots a framework that is already signed is one more sentence that describes the wrong path.
        if (!bootOnly) builtThisProcess = true;
        if (!bootOnly && GameAppMain.IdentityLine().Contains("MISSING"))
            Write("WARNING: no signing certificate in this app's Keychain — the chain will rewrite and assemble, "
                + "then fail at stage 4b. Open the Keys tab and tap \"Certificate (.p12)\" first if you want the boot.");
        // The request has now been honoured, whichever path wrote it: the picker writes gameapp.args as the run's
        // own record of its argv, the harness leaves it for the next launch to pick up. Renaming it aside HERE,
        // and not only in the launch path, is what makes the request truly one-shot: a run started from the picker
        // used to leave a LIVE one-shot request on disk, so reopening the app after that run ended — including
        // after the process was killed inside the guest — started the whole ~2.5-minute chain over with no tap
        // (measured on the device: "it quit, and on relaunch it started again"). The record survives as
        // gameapp.args.done.
        ConsumeArgs(Path.Combine(GameAppMain.Documents, "gameapp.args"));
        if (bootOnly)
        {
            // The pipeline reads this file and runs stage 5 alone: no profile, no rewrite, no assembly, no signing
            // — this launch exists to hand the guest a process that never did any of those, because the chain's
            // own ~2 GiB is live when it reaches the boot (measured) and a device has only ~520 MiB left of its
            // 3365 MiB ceiling. The pipeline marks it attempted before the boot and removes it once the boot has
            // come back, the shim when the boot ran to its time limit (Documents/gameapp.boot-only). A failed write
            // is said, not swallowed: without the file the
            // pipeline finds no request and re-runs the whole chain, which is not what this run claims to be.
            // An existing request is left as it is: re-arming an `attempted` one here would let a launch start it
            // by itself if this process died before the pipeline got to mark it again.
            try
            {
                if (!File.Exists(BootOnlyPath))
                    File.WriteAllText(BootOnlyPath, "boot the framework that is already built and signed\n");
            }
            catch (Exception error) { Write($"could not write {BootOnlyPath}: {error.GetType().Name}: {error.Message}"); }
        }
        running = true;
        chosen = name;
        // The library's selected game follows the run: the game being built, or the one whose framework is being booted.
        var runFacts = GatherFacts();
        var runGame = runFacts.Games != null && runFacts.Games.Contains(name) ? name
                    : bootOnly && !string.IsNullOrEmpty(runFacts.FrameworkGame) ? runFacts.FrameworkGame : null;
        if (runGame != null) selected = runGame;
        startedAt = DateTime.Now;
        StopWarm("a run starts");   // GameAppWarm.cs: the phone is the build's or the game's now
        PrepareGameWindow();
        // A marker the next launch reads: its presence means this run never finished, which is what the user saw
        // as "it still says running" (docs/PROFILE-DERIVED.md). Removed in Finish().
        try { File.WriteAllText(Path.Combine(GameAppMain.Documents, "gameapp.running"),
                                DateTime.Now.ToString("o") + " pid " + Environment.ProcessId + "\ngame=" + name + (bootOnly ? "\nbootonly=1" : "")); } catch { }
        RefreshScreen();   // running: the main button reads "Running…" and is disabled (GameAppUi.ChooseAction)
        StartTicker();
        if (!bootOnly) BackgroundBegin(runGame ?? name);   // GameAppBackground.cs: the build goes on if the user leaves the app
        StartThermalFile();   // the pipeline reads it: Low performance and a hot phone slow the jobs down
        stepStarts.Clear();
        stepStarts[0] = startedAt;
        lastRunGame = selected;
        buildFinished = false;
        failedRun = false;
        if (!bootOnly) ShowBuildScreen();

        var documents = GameAppMain.Documents;
        var bundle = NSBundle.MainBundle.BundlePath;
        var output = Path.Combine(documents, "extract");
        Write($"\n=== {chosen} ({(bootOnly ? "one process: dlopen → nnMain, on the framework this app already built and signed" : "one process: " + (skipExtract ? "chain on existing files" : "extract") + " → rewrite → assemble → zsign → dlopen → nnMain")})");
        Write($"output: {output}");
        Write($"free space before: {GameAppMain.FreeSpace()}");
        Write($"memory before extraction: {GameAppMain.MemLine()}");

        // The config the chain reads. The file is rebuilt every run, so a stale one can never be handed over.
        File.WriteAllText(Path.Combine(documents, "gameapp.config"),
                          GameAppMain.ConfigText(bundle, arguments != null && arguments.Length > 1
                                                             ? Path.GetFileName(arguments[1]) : null));
        // Tail from the END of the append-only logs: the screen shows THIS run, while the earlier runs' lines stay
        // on disk (the evidence a died run leaves) and travel with "Export logs".
        chainOffset = LengthOf(Path.Combine(documents, "gameapp.chain.log"));
        stagesOffset = LengthOf(Path.Combine(documents, "pipeline.stages.log"));
        lastStage = null;
        lastStageLogged = null;

        var previous = Console.Out;
        Console.SetOut(new GameAppLogWriter(text => BeginInvokeOnMainThread(() => Append(text))));
        Console.SetError(Console.Out);

        // BOTH halves run on this worker thread; only log updates are marshalled to the main thread (the ~3-minute
        // rewrite on the main thread froze the UI and invited the ~20 s watchdog kill).
        ThreadPool.QueueUserWorkItem(_ =>
        {
            int code = 0;
            string failure = null;
            var extractStarted = DateTime.Now;
            var converted = new List<string>();
            if (!skipExtract && arguments != null)
            {
                // An NSZ (a compressed NSP) first becomes the NSP it was made from: the extractor reads NSP and XCI only.
                // The NSP goes into Documents/.nsz/ (out of the game list) and is removed once the extraction is done.
                arguments = (string[]) arguments.Clone();
                for (int i = 1; i <= 2 && i < arguments.Length && code == 0; i++)
                {
                    var source = arguments[i];
                    if (source == "-" || !source.EndsWith(".nsz", StringComparison.OrdinalIgnoreCase)) continue;
                    var nsp = GameAppMain.NszToNsp(source, out var why);
                    if (nsp == null) { code = NszFailed; failure = why; break; }
                    arguments[i] = nsp;
                    converted.Add(nsp);
                }
                if (code == 0)
                try
                {
                    // The engine's own entry point, in this process, in this runtime. No shell, no argument file.
                    code = P.Main(arguments);
                    if (code == 0) ExtractDlc(name, arguments);
                }
                catch (Exception error)
                {
                    // An exception is a FAILURE, not a successful run: give it its own code so the exit line can
                    // never read "exit 0 (ok)". On the device a stale-path exception was reported as
                    // "extraction exit 0 (ok)" and the app walked into the chain with dead arguments.
                    code = ExtractionThrew;
                    failure = $"{error.GetType().FullName}: {error.Message}";
                }
            }
            var extractTook = DateTime.Now - extractStarted;
            foreach (var nsp in converted)
            {
                // The extraction wrote what the chain needs (extract/); the rebuilt NSP is as big as the game and only
                // a rebuild would read it again, which converts the .nsz again.
                try { File.Delete(nsp); } catch { }
            }

            Console.SetOut(previous);
            BeginInvokeOnMainThread(() =>
            {
                var meaning = code switch
                {
                    0 => "ok",
                    ExtractionThrew => "the extraction threw (see the line above)",
                    NszFailed => "the .nsz could not be turned back into an NSP (see the line above)",
                    2 => "usage: the tool refused its arguments",
                    5 => "the package is damaged (see the [RED] lines above)",
                    6 => "the keys cannot open this package",
                    _ => "see the lines above",
                };
                if (failure != null) Write($"\nextraction FAILED: {failure}");
                if (bootOnly) Write("\nextraction skipped: nothing is rebuilt — this launch boots the framework the app already signed");
                else if (skipExtract) Write("\nextraction skipped: chaining on the files already in Documents/extract");
                else
                {
                    Write($"\nextraction exit {code} ({meaning}), {Took(extractTook)}");
                    Write($"memory after extraction: {GameAppMain.MemLine()}");
                    Write($"free space after extraction: {GameAppMain.FreeSpace()}");
                }
                if (code != 0) { Write("the extraction failed; the chain is not started"); failedRun = true; }
            });
            if (code != 0)
            {
                BeginInvokeOnMainThread(Finish);
                return;
            }

            // The rest of the chain runs HERE, on the worker, in THIS process. The guest boots inside
            // gameapp_run_chain before the interpreter returns, and its own SIGTERM ends the process, so this
            // call may never come back -- which is fine: the evidence is already in the logs on disk.
            BeginInvokeOnMainThread(() => Write(bootOnly
                ? "\n--- boot only (same pid, off the main thread): dlopen → nnMain, in a process that ran no stage of the chain"
                : "\n--- chain (same pid, off the main thread): profile → rewrite → assemble → zsign → dlopen → nnMain"));
            var chainStarted = DateTime.Now;
            int rc = -1;
            string chainError = null;
            pythonRanThisProcess = true;
            try { rc = GameAppMain.gameapp_run_chain(); }
            catch (Exception error) { chainError = $"{error.GetType().FullName}: {error.Message}"; }
            var chainTook = DateTime.Now - chainStarted;
            BeginInvokeOnMainThread(() =>
            {
                if (chainError != null) Write($"chain exception: {chainError}");
                Write($"chain returned {rc} after {Took(chainTook)}");
                failedRun = rc != 0 || chainError != null;
                Write($"memory at the end: {GameAppMain.MemLine()}");
                Write($"free space at the end: {GameAppMain.FreeSpace()}");
                if (!RestartForStage3(rc, chainError)) Finish();   // GameAppResume.cs: the chain may end on purpose before stage 3
            });
        });
    }

    private void Finish()
    {
        TailLogs();   // a build that stops within a second ends before the first tick: its last lines and step come now
        running = false;
        try { File.Delete(Path.Combine(GameAppMain.Documents, "gameapp.running")); } catch { }
        DismissGameWindow();
        lastRunEnd = DateTime.Now;
        StopTicker();
        buildFinished = true;
        BackgroundEnd(!failedRun);   // GameAppBackground.cs
        StopThermalFile();
        NotifyBuildEnded();
        QueueBuildEnded();
        RefreshRunLine();
        RefreshBuildScreen();
        // The run may have left a boot request (a refused in-process boot, or a boot that never came back): the
        // main button now says what is owed, and the checklist shows the framework that is waiting.
        RefreshScreen();
        if (!failedRun) StartWarm(lastRunGame ?? selected);   // GameAppWarm.cs: prepare the shaders so the first session hits Metal's cache
    }

    private static string Took(TimeSpan elapsed) =>
        elapsed.TotalSeconds < 1 ? $"{elapsed.TotalMilliseconds:F0}ms" : $"{elapsed.TotalSeconds:F1}s";

    private StreamWriter uiLog;

    private long uiLogNextTrim = LogWindow.UiLogTrimAbove;

    /// Keep gameapp.ui.log bounded without cutting what a log share needs: past 64 MB it is rewritten from the start of
    /// the last 10 minutes (LogWindow), never inside them. When nothing older exists the file just grows on and is
    /// looked at again 16 MB later. Called from Append on the main thread, so the StreamWriter is not shared.
    private void RotateUiLog()
    {
        try
        {
            uiLog.Flush();
            uiLog.Dispose();
            uiLog = null;
            LogWindow.TrimToWindow(GameAppMain.Documents, "gameapp.ui.log", DateTime.UtcNow);
            var path = Path.Combine(GameAppMain.Documents, "gameapp.ui.log");
            uiLogNextTrim = new FileInfo(path).Length + (16L << 20);
            uiLog = new StreamWriter(path, append: true) { AutoFlush = true };
        }
        catch { }
    }

    private void Write(string text) => Append(text + (text.EndsWith("\n") ? "" : "\n"));

    private void StartTicker()
    {
        StopTicker();
        progress.Progress = 0;
        progress.Hidden = false;
        ticker = new Timer(_ => BeginInvokeOnMainThread(() =>
        {
            if (!running) return;
            // A moving clock on the run card and the build screen cannot be mistaken for a frozen run
            // (docs/PROFILE-DERIVED.md).
            RefreshBuildScreen();
            RefreshRunLine();
            // The bar follows the stage and never goes back inside one run.
            BackgroundTick();   // GameAppBackground.cs: the system's progress UI
            var target = (float) GameAppUi.ParseStage(lastStage).Fraction;
            if (target > progress.Progress) progress.SetProgress(target, true);
        }), null, TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(1));
    }

    private void StopTicker()
    {
        ticker?.Dispose();
        ticker = null;
        if (!running)
        {
            progress.Hidden = true;
        }
    }

    /// Append whatever the chain's own logs have gained since the last tick. The chain is CPython: its prints
    /// go to files beside Documents, not to this process' standard output.
    private void TailLogs()
    {
        chainOffset = Tail(Path.Combine(GameAppMain.Documents, "gameapp.chain.log"), chainOffset);
        stagesOffset = Tail(Path.Combine(GameAppMain.Documents, "pipeline.stages.log"), stagesOffset);
        // The chain overwrites Documents/gameapp.stage on every step: append the new one to the log and keep the
        // latest in lastStage, which the run card and the build screen show.
        try
        {
            var path = Path.Combine(GameAppMain.Documents, "gameapp.stage");
            if (!File.Exists(path)) return;
            // While a run is going, only a stage THIS run wrote counts. The file still holds the previous run's last
            // line (a boot, say) until the chain's first step overwrites it, and the extraction runs before that:
            // read as it stands, the bar would sit at the old run's mark from the first second.
            if (running && !GameAppUi.IsStageFromThisRun(File.GetLastWriteTime(path), startedAt)) return;
            var text = File.ReadAllText(path).Trim();
            if (string.IsNullOrEmpty(text) || text == lastStageLogged) return;
            lastStageLogged = text;
            lastStage = text;
            NoteStep(text);
            Write(text);
            ShowGameWindowAtBoot();
        }
        catch { }
    }

    /// The current length of a log file, for starting a tail at its end (0 when it does not exist yet).
    private static long LengthOf(string path)
    {
        try { return File.Exists(path) ? new FileInfo(path).Length : 0; }
        catch { return 0; }
    }

    private long Tail(string path, long offset)
    {
        try
        {
            if (!File.Exists(path)) return offset;
            using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite))
            {
                if (offset > stream.Length) offset = 0;
                stream.Seek(offset, SeekOrigin.Begin);
                // leaveOpen: `stream.Position` is read BELOW while the stream is still open; read after the reader's
                // using block it throws, and the 1 Hz tailer then re-appended the same window forever.
                using (var reader = new StreamReader(stream, Encoding.UTF8,
                                                     detectEncodingFromByteOrderMarks: false, bufferSize: 4096,
                                                     leaveOpen: true))
                {
                    var text = reader.ReadToEnd();
                    var next = stream.Position;   // the new offset, taken while the stream is still open
                    // Append only real, NEW content: an identical consecutive window is never replayed, whatever
                    // the reason it was read again.
                    if (!string.IsNullOrEmpty(text) && text != lastTailWindow)
                    {
                        lastTailWindow = text;
                        Append(text);
                    }
                    return next;
                }
            }
        }
        catch (Exception error)
        {
            // A tail failure must never look like "nothing new" and must never replay: skip past whatever the file
            // holds now, and say so once, rather than returning the old offset (which is what re-appended forever).
            try
            {
                Write($"tail of {Path.GetFileName(path)} failed: {error.GetType().Name}: {error.Message}");
                return new FileInfo(path).Length;
            }
            catch { return offset; }
        }
    }

    /// Share the log FILES through UIActivityViewController (AirDrop, Mail, Files), so a run that misbehaved can
    /// leave the device without the user copying text off the screen -- which they could not do when the app
    /// stopped responding. The logs are kept append-only across launches, so a run that died still has its lines.
    private void ExportLogs()
    {
        var urls = new List<NSUrl>();
        foreach (var name in new[] { "gameapp.ui.log", "pipeline.stages.log", "gameapp.chain.log", "gameapp.stage",
                                     "gameapp.guest.log" })
        {
            var path = Path.Combine(GameAppMain.Documents, name);
            if (File.Exists(path)) urls.Add(NSUrl.FromFilename(path));
        }
        if (urls.Count == 0) { Write("Export logs: nothing to export yet"); return; }
        Write($"Export logs: sharing {urls.Count} file(s) from {GameAppMain.Documents}");
        var activity = new UIActivityViewController(urls.ToArray(), null);
        Theme.Anchor(activity, export, View);
        PresentViewController(activity, true, null);
    }

    /// A relaunch after a run that never finished must say so, not keep claiming to be running: the app writes
    /// Documents/gameapp.running at the start of a run and removes it in Finish(), so its presence at launch is
    /// "the previous run was interrupted" -- except for the one end that is by design, a boot that ran to its time
    /// limit and whose graceful exit finished (GameAppUi.ClassifyPreviousRun): that one is said in a single line,
    /// without the banner. The last [mem:...] line and the stage it had reached are the two facts that make a real
    /// interruption diagnosable, and the append-only logs still hold them (docs/PROFILE-DERIVED.md).
    /// Returns true when it reported an interruption, so the caller knows NOT to auto-run (one run per explicit action).
    private bool ReportInterruptedRunIfAny()
    {
        var marker = Path.Combine(GameAppMain.Documents, "gameapp.running");
        if (!File.Exists(marker)) return false;
        running = false;   // never inherit a stale "running" state
        string started = "";
        string markerText = "";
        try { markerText = File.ReadAllText(marker); started = markerText.Split('\n')[0].Trim(); } catch { }
        // How did it end? The chain log's last line separates the designed end from a fault or a kill (the shim logs
        // the time limit before it signals itself, and names a fatal signal before it re-raises; a kill is SIGKILL
        // and writes nothing), and the guest log's last line says whether the exit then finished. Reading this wrong
        // reports the designed end as a crash, or the other way round -- which a whole device run was lost to. The
        // chain log is read WITHOUT the launch lines this process's own constructor has just appended
        // (GameAppUi.LastRunLine): they would stand in for the previous run's last line.
        var stageLine = LastLine("gameapp.stage");
        string chainLast = GameAppUi.LastRunLine(ReadLinesOrEmpty("gameapp.chain.log"));
        string guestLast = LastLine("gameapp.guest.log");
        var kind = GameAppUi.ClassifyPreviousRun(stageLine, chainLast, guestLast == "(none)" ? null : guestLast);
        if (kind == GameAppUi.PreviousRun.DesignedEnd || kind == GameAppUi.PreviousRun.PlayerEnd)
        {
            // Every boot that runs to its time limit, or that the player ends with Quit game, ends this way (the
            // process ends by SIGTERM, Finish never runs): not an interruption, and a banner that says so every time
            // teaches the user to ignore it.
            Write("");
            Write(kind == GameAppUi.PreviousRun.PlayerEnd
                ? $"Previous run ended when you quit the game — the designed end, and the guest's exit completed (started {started})."
                : $"Previous run ended at its boot time limit — the designed end, and the guest's exit completed (started {started}).");
            try { File.Delete(marker); } catch { }
            return false;
        }
        Write("");
        Write($"PREVIOUS RUN WAS INTERRUPTED — it left Documents/gameapp.running behind (started {started}).");
        Write($"  last stage reached: {stageLine}");
        Write($"  last memory line:   {LastMatching("pipeline.stages.log", "[mem:")}");
        RecordKilledBuild(markerText, stageLine, started);   // GameAppResume.cs: the library offers to resume it
        if (stageLine.Contains("booting the guest"))
        {
            if (kind == GameAppUi.PreviousRun.DesignedEndWithoutCleanExit)
                Write($"  the boot ended by the shim's own signal (its time limit, or Quit game) — \"{chainLast}\" — but the guest's last line is "
                    + $"\"{guestLast}\", not `graceful exit done`: the exit did not finish (it hung, or the process was killed during it). "
                    + "That is not the clean designed end; gameapp.guest.log holds what the guest said last.");
            else if (kind == GameAppUi.PreviousRun.FaultInGuest)
                Write($"  the guest died on a signal, and the shim named it before re-raising — \"{chainLast}\": that "
                    + "is a fault INSIDE the guest, not a kill from outside. Its backtrace is in Analytics Data "
                    + "(the GameApp .ips); gameapp.guest.log holds the guest's own last words.");
            else
                Write("  the chain log stops at the boot call — no `dm_host_probe_boot returned`, no signal line: a "
                    + "kill while the guest runs writes nothing (jetsam and the watchdog are SIGKILL). The memory "
                    + "line above is what the guest had to work with; Analytics Data carries the verdict (a "
                    + "JetsamEvent, or the GameApp .ips).");
        }
        Write("  the logs are kept across launches — use Export logs to get them off the device.");
        try { File.Delete(marker); } catch { }
        // The interrupted run already consumed its request: drop the stale argv so a later launch cannot run it
        // (this is the other half of "do not start again by itself").
        try { File.Delete(Path.Combine(GameAppMain.Documents, "gameapp.args")); } catch { }
        return true;
    }

    /// Every line of a log in Documents; none when it does not exist. A failure to read is said, not swallowed.
    private string[] ReadLinesOrEmpty(string name)
    {
        try
        {
            var path = Path.Combine(GameAppMain.Documents, name);
            return File.Exists(path) ? File.ReadAllLines(path) : Array.Empty<string>();
        }
        catch (Exception error)
        {
            Write($"(could not read {name}: {error.GetType().Name}: {error.Message})");
            return Array.Empty<string>();
        }
    }

    private string LastLine(string name)
    {
        try
        {
            var path = Path.Combine(GameAppMain.Documents, name);
            if (!File.Exists(path)) return "(none)";
            var lines = File.ReadAllLines(path).Where(l => !string.IsNullOrWhiteSpace(l)).ToArray();
            return lines.Length == 0 ? "(none)" : lines[lines.Length - 1];
        }
        catch (Exception error) { return "(unreadable: " + error.GetType().Name + ")"; }
    }

    private string LastMatching(string name, string needle)
    {
        try
        {
            var path = Path.Combine(GameAppMain.Documents, name);
            if (!File.Exists(path)) return "(none)";
            var line = File.ReadAllLines(path).LastOrDefault(l => l.Contains(needle));
            return line ?? "(none)";
        }
        catch (Exception error) { return "(unreadable: " + error.GetType().Name + ")"; }
    }

    private void ImportCertificate()
    {
        // Any file type (a provider may type a .p12 otherwise); the file is checked after the pick
        // (GameAppUi.CertificateFileProblem, IsPkcs12). IMPORT mode (a copy), not Open: in a re-signed install Open
        // mode did nothing (2026-11-18). The copy lands in tmp/ and is read and deleted at once (AskCertificatePassword).
        var picker = new UIDocumentPickerViewController(new[] { "com.rsa.pkcs-12", "public.data", "public.item" }, UIDocumentPickerMode.Import);
        picker.Delegate = new GameAppCertificateDelegate(this);
        PresentViewController(picker, true, null);
    }

    /// The password is asked for once, after the file, and never shown or logged again: it lives in the same
    /// Keychain item as the .p12 from that moment on.
    /// Presents `vc` once nothing else is on screen. The document picker calls its delegate while it is still going
    /// away, and an alert presented then is dropped by UIKit without a word ("Attempt to present ... while a
    /// presentation is in progress"): the certificate's password prompt never showed and picking a .p12 looked like it
    /// did nothing (reported with a FlareStore certificate). Waits up to ~3 s, then closes a picker still standing.
    private void PresentWhenFree(UIViewController vc, int tries = 20)
    {
        // On the top-most screen that stays (the build screen, a sheet), once nothing is on its way out: an alert
        // presented while a picker is being dismissed is dropped by UIKit (trap 56).
        UIViewController top = this;
        while (top.PresentedViewController != null && !top.PresentedViewController.IsBeingDismissed) top = top.PresentedViewController;
        if (top.PresentedViewController != null && tries > 0) { NSTimer.CreateScheduledTimer(0.15, _ => PresentWhenFree(vc, tries - 1)); return; }
        top.PresentViewController(vc, true, null);
    }

    /// The picked file's bytes, read right away; a copy the picker made in this app's container is deleted there and
    /// then, so the certificate never stays in a file of ours (it lives only in memory until the password is typed).
    private static byte[] TakeCertificateBytes(NSUrl url, out string why)
    {
        why = null;
        var path = url?.Path;
        if (string.IsNullOrEmpty(path)) { why = "the picker returned no path"; return null; }
        bool scoped = url.StartAccessingSecurityScopedResource();
        try { return File.ReadAllBytes(path); }
        catch (Exception error) { why = $"{error.GetType().Name}: {error.Message}"; return null; }
        finally
        {
            if (scoped) url.StopAccessingSecurityScopedResource();
            var home = Environment.GetEnvironmentVariable("HOME") ?? "";
            if (home.Length > 0 && path.StartsWith(home, StringComparison.Ordinal))   // a copy inside our own container
                try { File.Delete(path); } catch (Exception) { }
        }
    }

    private void AskCertificatePassword(NSUrl url)
    {
        var name = url?.LastPathComponent ?? "certificate";
        Write($"certificate: picked {name}");
        var bytes = TakeCertificateBytes(url, out var readWhy);
        var problem = GameAppUi.CertificateFileProblem(name)
                      ?? (bytes == null ? $"{name} could not be read: {readWhy}" : null)
                      ?? (!GameAppUi.IsPkcs12(bytes) ? $"{name} is not a .p12 (PKCS#12) file." : null);
        if (problem != null)
        {
            if (bytes != null) Array.Clear(bytes, 0, bytes.Length);
            Write("certificate: " + problem);
            var refuse = UIAlertController.Create("Not a certificate", problem, UIAlertControllerStyle.Alert);
            refuse.AddAction(UIAlertAction.Create("OK", UIAlertActionStyle.Cancel, null));
            PresentWhenFree(refuse);
            return;
        }
        var alert = UIAlertController.Create("Signing certificate",
            $"{name}: the password is stored in this app's Keychain together with the .p12, and it cannot be shown again. "
            + "Nothing is written to a file.", UIAlertControllerStyle.Alert);
        alert.AddTextField(field =>
        {
            field.SecureTextEntry = true;
            field.Placeholder = ".p12 password";
            field.AutocapitalizationType = UITextAutocapitalizationType.None;
            field.AutocorrectionType = UITextAutocorrectionType.No;
        });
        alert.AddAction(UIAlertAction.Create("Cancel", UIAlertActionStyle.Cancel, _ =>
        {
            Array.Clear(bytes, 0, bytes.Length);
            Write("certificate: not stored (cancelled)");
        }));
        alert.AddAction(UIAlertAction.Create("Store in the Keychain", UIAlertActionStyle.Default,
            _ => StoreCertificate(bytes, alert.TextFields[0].Text)));
        PresentWhenFree(alert);
    }

    private void StoreCertificate(byte[] bytes, string passphrase)
    {
        try
        {
            try
            {
                int rc = GameAppMain.IdentityStore(bytes, passphrase);
                if (rc == 0)
                    Write($"certificate stored: {bytes.Length} byte .p12 in this app's Keychain "
                        + "(unlocked-this-device-only; no file, no log, not printed again)");
                else if (rc == 1)
                    Write($"certificate stored: {bytes.Length} byte .p12 in a protected file — this install has no Keychain access "
                        + "(the installer signed it without one, errSecMissingEntitlement -34018); the file is unreadable while the phone is locked and is not backed up");
                else if (rc == -34018)
                    Write("certificate NOT stored (-34018): this install has no Keychain access and the protected file could not be written either");
                else
                    Write($"certificate NOT stored (rc={rc}): the Keychain refused the write — the .p12 may be unreadable "
                        + "or the item could not be replaced; the chain will fail at stage 4b until this succeeds");
            }
            finally { Array.Clear(bytes, 0, bytes.Length); }
        }
        catch (Exception error) { Write($"certificate import failed: {error.GetType().Name}: {error.Message}"); }
        Write(GameAppMain.IdentityLine());
        RefreshScreen();
    }

    private sealed class GameAppCertificateDelegate : NSObject, IUIDocumentPickerDelegate
    {
        private readonly GameAppViewController owner;
        public GameAppCertificateDelegate(GameAppViewController owner) => this.owner = owner;
        public void DidPickDocument(UIDocumentPickerViewController controller, NSUrl url) => owner.AskCertificatePassword(url);
        public void DidPickDocumentAtUrls(UIDocumentPickerViewController controller, NSUrl[] urls)
        {
            if (urls != null && urls.Length > 0) owner.AskCertificatePassword(urls[0]);
            else owner.Write("certificate: the picker returned no file");
        }
        [Export("documentPickerWasCancelled:")]
        public void WasCancelled(UIDocumentPickerViewController controller) => owner.Write("certificate: picker closed without a file");
    }

    private void Import()
    {
        var picker = new UIDocumentPickerViewController(new[] { "public.item" }, UIDocumentPickerMode.Import);
        picker.AllowsMultipleSelection = true;   // a game with its update and DLC in one go
        picker.Delegate = new GameAppImportDelegate(this);
        PresentViewController(picker, true, null);
    }

    /// Move rather than copy when the file is already inside our container (Import mode puts it in tmp/): a
    /// 1.4 GB dump would otherwise need twice its size in free space.
    private void CopyIn(NSUrl url)
    {
        var path = url?.Path;
        if (string.IsNullOrEmpty(path)) return;
        try
        {
            var name = url.LastPathComponent;
            var destination = Path.Combine(GameAppMain.Documents, name);
            if (File.Exists(destination)) File.Delete(destination);
            try { File.Move(path, destination); }
            catch (IOException) { File.Copy(path, destination, true); }
            Write($"imported: {name}");
            RefreshGames();
            RefreshScreen();
        }
        catch (Exception error) { Write($"import failed: {error.GetType().Name}: {error.Message}"); }
    }

    private sealed class GameAppImportDelegate : NSObject, IUIDocumentPickerDelegate
    {
        private readonly GameAppViewController owner;
        public GameAppImportDelegate(GameAppViewController owner) => this.owner = owner;
        public void DidPickDocument(UIDocumentPickerViewController controller, NSUrl url) => owner.CopyIn(url);
        public void DidPickDocumentAtUrls(UIDocumentPickerViewController controller, NSUrl[] urls)
        {
            foreach (var url in urls) owner.CopyIn(url);
        }
    }
}

/// A missing prerequisite on the Games page: the checklist row's own label on a tinted bar, a chevron, one tap to the importer.
public sealed class BannerView : UIControl
{
    public BannerView(UILabel label, UIColor tint)
    {
        BackgroundColor = tint.ColorWithAlpha(0.15f);
        Layer.CornerRadius = 12;
        IsAccessibilityElement = true;
        AccessibilityTraits = UIAccessibilityTrait.Button;
        label.UserInteractionEnabled = false;
        var chevron = new UIImageView(UIImage.GetSystemImage("chevron.right")) { TintColor = UIColor.SecondaryLabel, UserInteractionEnabled = false };
        chevron.SetContentHuggingPriority(1000, UILayoutConstraintAxis.Horizontal);
        var stack = new UIStackView(new UIView[] { label, chevron })
        {
            Axis = UILayoutConstraintAxis.Horizontal,
            Alignment = UIStackViewAlignment.Center,
            Spacing = 8,
            UserInteractionEnabled = false,
            TranslatesAutoresizingMaskIntoConstraints = false,
        };
        AddSubview(stack);
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            stack.TopAnchor.ConstraintEqualTo(TopAnchor, 10),
            stack.LeadingAnchor.ConstraintEqualTo(LeadingAnchor, 12),
            stack.TrailingAnchor.ConstraintEqualTo(TrailingAnchor, -12),
            stack.BottomAnchor.ConstraintEqualTo(BottomAnchor, -10),
        });
    }
}

/// The game's settings sheet (the gear). Each change is handed to `changed` at once, which writes
/// Documents/games/<base title id>/settings.tsv; there is no Save button to forget. `rebuild` is null unless a rebuild
/// from scratch could start. The memory line is read-only: the app sizes the game's memory from its first boot.
public sealed class GameAppSettingsViewController : UIViewController
{
    private readonly string name;
    private readonly bool enabled;
    private GameAppUi.GameSettings settings;
    private readonly Action<GameAppUi.GameSettings> changed;
    private readonly Action rebuild;
    // The engine launcher's rows (Documents/games/<id>/launcher.json), when the game was built: the sheet is drawn from them.
    private IReadOnlyList<GameAppUi.LauncherRow> launcher;
    private Dictionary<string, string> values;
    private Action<Dictionary<string, string>> valuesChanged;
    private readonly List<(GameAppUi.LauncherRow Row, UIView View)> launcherViews = new List<(GameAppUi.LauncherRow, UIView)>();
    private readonly List<(string Section, UIView Header)> sectionHeaders = new List<(string, UIView)>();
    private readonly List<Action> refreshers = new List<Action>();

    /// Draws the sheet from the launcher's rows instead of the four fixed ones. Call before the sheet is shown.
    public void UseLauncher(IReadOnlyList<GameAppUi.LauncherRow> rows, Dictionary<string, string> current, Action<Dictionary<string, string>> save)
    {
        launcher = rows;
        values = current ?? new Dictionary<string, string>();
        valuesChanged = save;
    }

    /// "Delete this game…": set by the screen; the sheet closes, then the screen asks for confirmation.
    public Action DeleteGame { get; set; }

    /// "Export as .ipa…": set by the screen for the built game; the sheet closes, then the screen asks.
    public Action ExportGame { get; set; }

    /// "Saves…": the game's copies (GameAppSavesUi.cs); the sheet closes, then the screen shows them.
    public Action ManageSaves { get; set; }

    /// "Mods & cheats…": Atmosphere-layout mods, 60 fps patches and cheats (GameAppModsUi.cs); set for a game in iBuilder.
    public Action ManageMods { get; set; }

    /// The game's update and DLC, as lines, and "Add an update or DLC…" (the screen's importer): set for a game in iBuilder.
    public string ContentInfo { get; set; }
    public Action AddContent { get; set; }

    public GameAppSettingsViewController(string name, bool enabled, GameAppUi.GameSettings settings,
                                         Action<GameAppUi.GameSettings> changed, Action rebuild)
    {
        this.name = name;
        this.enabled = enabled;
        this.settings = settings;
        this.changed = changed;
        this.rebuild = rebuild;
    }

    private static UILabel Text(string text, UIFontTextStyle style, UIColor color, bool bold = false)
    {
        var font = UIFont.GetPreferredFontForTextStyle(style);
        if (bold) font = UIFont.FromDescriptor(font.FontDescriptor.CreateWithTraits(UIFontDescriptorSymbolicTraits.Bold), font.PointSize);
        return new UILabel { Text = text, Font = font, TextColor = color, Lines = 0, AdjustsFontForContentSizeCategory = true };
    }

    /// A titled row of choices: `labels` are what the user reads, `values` what the file keeps.
    private UIView Choice(string title, string id, string[] labels, string[] values, string current, Action<string> set)
    {
        var control = new UISegmentedControl(labels) { Enabled = enabled, AccessibilityIdentifier = id };
        var index = Array.IndexOf(values, current);
        control.SelectedSegment = index < 0 ? 0 : index;
        control.ValueChanged += (_, __) => set(values[(int) control.SelectedSegment]);
        return new UIStackView(new UIView[] { Text(title, UIFontTextStyle.Subheadline, UIColor.Label, bold: true), control })
        {
            Axis = UILayoutConstraintAxis.Vertical,
            Spacing = 6,
        };
    }

    public override void ViewDidLoad()
    {
        base.ViewDidLoad();
        View.BackgroundColor = UIColor.SystemBackground;

        var done = UIButton.FromType(UIButtonType.System);
        done.SetTitle("Done", UIControlState.Normal);
        done.AccessibilityIdentifier = "settings-done";
        done.TouchUpInside += (_, __) => DismissViewController(true, null);
        var header = new UIStackView(new UIView[] { Text("Game settings", UIFontTextStyle.Title3, UIColor.Label, bold: true), new UIView(), done })
        {
            Axis = UILayoutConstraintAxis.Horizontal,
            Alignment = UIStackViewAlignment.Center,
        };

        var views = new List<UIView> { header, Text(name, UIFontTextStyle.Subheadline, UIColor.SecondaryLabel) };
        if (!enabled)
            views.Add(Text("This file name carries no [title id], so its settings cannot be saved. Rename the file to include it.",
                           UIFontTextStyle.Footnote, UIColor.SystemOrange));
        if (launcher != null) AddLauncherViews(views);
        else
        {
            views.Add(Choice("Mode", "setting-mode", new[] { "Automatic", "Handheld", "Docked" }, new[] { "auto", "handheld", "docked" },
                             settings.Mode, v => Apply(settings with { Mode = v })));
            views.Add(Choice("Frame rate", "setting-fps", new[] { "Game's own", "30", "60" }, new[] { "auto", "30", "60" },
                             settings.Fps, v => Apply(settings with { Fps = v })));
            views.Add(Choice("Picture", "setting-picture", new[] { "Fit", "Fill screen" }, new[] { "fit", "fill" },
                             settings.Picture, v => Apply(settings with { Picture = v })));

            var toggle = new UISwitch { On = settings.ShowFps, Enabled = enabled, AccessibilityIdentifier = "setting-show-fps" };
            toggle.ValueChanged += (_, __) => Apply(settings with { ShowFps = toggle.On });
            var fpsRow = new UIStackView(new UIView[] { Text("Show FPS", UIFontTextStyle.Subheadline, UIColor.Label, bold: true), new UIView(), toggle })
            {
                Axis = UILayoutConstraintAxis.Horizontal,
                Alignment = UIStackViewAlignment.Center,
            };
            views.Add(fpsRow);
        }

        var memory = new UIStackView(new UIView[]
        {
            Text("Game memory: Automatic", UIFontTextStyle.Subheadline, UIColor.Label, bold: true),
            Text("The app sizes it from the game's first boot.", UIFontTextStyle.Footnote, UIColor.SecondaryLabel),
        })
        {
            Axis = UILayoutConstraintAxis.Vertical,
            Spacing = 2,
        };
        views.Add(memory);

        if (rebuild != null)
        {
            var button = UIButton.FromType(UIButtonType.System);
            var config = UIButtonConfiguration.TintedButtonConfiguration;
            config.Title = "Rebuild from scratch";
            config.BaseForegroundColor = UIColor.SystemRed;
            button.Configuration = config;
            button.AccessibilityIdentifier = "rebuild-from-scratch";
            button.TouchUpInside += (_, __) => DismissViewController(true, () => rebuild());
            views.Add(button);
            views.Add(Text(GameAppUi.RebuildTitle.Replace("Rebuild everything instead", "Rebuilds everything for this game") + ".",
                           UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        }

        if (AddContent != null)
        {
            views.Add(Text("Update and DLC", UIFontTextStyle.Subheadline, UIColor.Label, bold: true));
            views.Add(Text(ContentInfo ?? "", UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
            var add = UIButton.FromType(UIButtonType.System);
            var aconfig = UIButtonConfiguration.TintedButtonConfiguration;
            aconfig.Title = "Add an update or DLC…";
            aconfig.Image = UIImage.GetSystemImage("plus.square.on.square");
            aconfig.ImagePadding = 6;
            add.Configuration = aconfig;
            add.AccessibilityIdentifier = "add-content";
            var addAction = AddContent;
            add.TouchUpInside += (_, __) => DismissViewController(true, () => addAction());
            views.Add(add);
        }

        if (ManageSaves != null)
        {
            var saves = UIButton.FromType(UIButtonType.System);
            var sconfig = UIButtonConfiguration.TintedButtonConfiguration;
            sconfig.Title = "Saves…";
            sconfig.Image = UIImage.GetSystemImage("externaldrive");
            sconfig.ImagePadding = 6;
            saves.Configuration = sconfig;
            saves.AccessibilityIdentifier = "manage-saves";
            var savesAction = ManageSaves;
            saves.TouchUpInside += (_, __) => DismissViewController(true, () => savesAction());
            views.Add(saves);
            views.Add(Text("Copies of the game's save: one before each launch, yours when you want; restore, share, and JKSV backups in and out.",
                           UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        }

        if (ManageMods != null)
        {
            var modsButton = UIButton.FromType(UIButtonType.System);
            var mconfig = UIButtonConfiguration.TintedButtonConfiguration;
            mconfig.Title = "Mods & cheats…";
            mconfig.Image = UIImage.GetSystemImage("wand.and.stars");
            mconfig.ImagePadding = 6;
            modsButton.Configuration = mconfig;
            modsButton.AccessibilityIdentifier = "manage-mods";
            var modsAction = ManageMods;
            modsButton.TouchUpInside += (_, __) => DismissViewController(true, () => modsAction());
            views.Add(modsButton);
            views.Add(Text("Mods in the Atmosphere layout (RomFS files, code patches such as 60 fps, cheats), from a .zip or a folder in Files. "
                           + "A 60 fps mod usually goes with the FPS setting \"60\" above.", UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        }

        if (ExportGame != null)
        {
            var export = UIButton.FromType(UIButtonType.System);
            var econfig = UIButtonConfiguration.TintedButtonConfiguration;
            econfig.Title = "Export as .ipa…";
            econfig.Image = UIImage.GetSystemImage("square.and.arrow.up");
            econfig.ImagePadding = 6;
            export.Configuration = econfig;
            export.AccessibilityIdentifier = "export-game";
            var exportAction = ExportGame;
            export.TouchUpInside += (_, __) => DismissViewController(true, () => exportAction());
            views.Add(export);
            views.Add(Text("Makes this built game an app of its own, to install with SideStore or FlareStore. It runs without this app.",
                           UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        }

        if (DeleteGame != null)
        {
            var delete = UIButton.FromType(UIButtonType.System);
            var dconfig = UIButtonConfiguration.PlainButtonConfiguration;
            dconfig.Title = "Delete this game…";
            dconfig.Image = UIImage.GetSystemImage("trash");
            dconfig.ImagePadding = 6;
            dconfig.BaseForegroundColor = UIColor.SystemRed;
            delete.Configuration = dconfig;
            delete.AccessibilityIdentifier = "delete-game";
            var action = DeleteGame;
            delete.TouchUpInside += (_, __) => DismissViewController(true, () => action());
            views.Add(delete);
            views.Add(Text("Frees the space the game takes: its file, its shaders and its build. You choose whether its saves stay.",
                           UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        }

        var stack = new UIStackView(views.ToArray()) { Axis = UILayoutConstraintAxis.Vertical, Spacing = 16, TranslatesAutoresizingMaskIntoConstraints = false };
        var scroll = new UIScrollView { TranslatesAutoresizingMaskIntoConstraints = false };
        scroll.AddSubview(stack);
        View.AddSubview(scroll);
        var guide = View.SafeAreaLayoutGuide;
        NSLayoutConstraint.ActivateConstraints(new[]
        {
            scroll.TopAnchor.ConstraintEqualTo(View.TopAnchor),
            scroll.BottomAnchor.ConstraintEqualTo(guide.BottomAnchor),
            scroll.LeadingAnchor.ConstraintEqualTo(guide.LeadingAnchor),
            scroll.TrailingAnchor.ConstraintEqualTo(guide.TrailingAnchor),
            stack.TopAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.TopAnchor, 20),
            stack.BottomAnchor.ConstraintEqualTo(scroll.ContentLayoutGuide.BottomAnchor, -20),
            stack.LeadingAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.LeadingAnchor, 20),
            stack.TrailingAnchor.ConstraintEqualTo(scroll.FrameLayoutGuide.TrailingAnchor, -20),
        });
    }

    private void Apply(GameAppUi.GameSettings next)
    {
        settings = next;
        changed(next);
    }

    // ---- the sheet drawn from launcher.json -------------------------------------------------------------------

    private void AddLauncherViews(List<UIView> views)
    {
        foreach (var (section, rows) in GameAppUi.Sections(launcher))
        {
            var header = Text(section, UIFontTextStyle.Headline, UIColor.Label, bold: true);
            sectionHeaders.Add((section, header));
            views.Add(header);
            foreach (var row in rows)
            {
                var view = LauncherRowView(row);
                launcherViews.Add((row, view));
                views.Add(view);
            }
        }
        RefreshVisibility();
    }

    private void RefreshVisibility()
    {
        foreach (var (row, view) in launcherViews) view.Hidden = !GameAppUi.IsVisible(launcher, row, values);
        foreach (var (section, header) in sectionHeaders)
            header.Hidden = !launcherViews.Any(v => v.Row.Section == section && !v.View.Hidden);
        foreach (var refresh in refreshers) refresh();
    }

    private void SetValue(string env, string value)
    {
        values[env] = value;
        valuesChanged?.Invoke(values);
        RefreshVisibility();
    }

    private UIView LauncherRowView(GameAppUi.LauncherRow row)
    {
        var id = "setting-" + row.Env;
        var current = GameAppUi.ValueOf(row, values);
        var title = Text(row.Label, UIFontTextStyle.Subheadline, UIColor.Label, bold: true);
        var parts = new List<UIView>();
        switch (row.Kind)
        {
            case "toggle":
            {
                var toggle = new UISwitch { On = current == row.On, Enabled = enabled, AccessibilityIdentifier = id };
                toggle.ValueChanged += (_, __) => SetValue(row.Env, toggle.On ? row.On : row.Off);
                parts.Add(new UIStackView(new UIView[] { title, new UIView(), toggle })
                {
                    Axis = UILayoutConstraintAxis.Horizontal,
                    Alignment = UIStackViewAlignment.Center,
                    Spacing = 8,
                });
                break;
            }
            case "range":
            {
                var slider = new UISlider
                {
                    MinValue = (float) row.Min, MaxValue = (float) row.Max, Value = (float) GameAppUi.RangeValue(row, values),
                    Enabled = enabled, AccessibilityIdentifier = id,
                };
                var shown = Text("", UIFontTextStyle.Subheadline, UIColor.SecondaryLabel);
                shown.SetContentHuggingPriority(1000, UILayoutConstraintAxis.Horizontal);
                void Show()
                {
                    var text = GameAppUi.SnapRange(row, slider.Value);
                    var px = GameAppUi.RenderSizeText(row, double.Parse(text, System.Globalization.CultureInfo.InvariantCulture),
                                                      values.TryGetValue("DM_OPMODE", out var m) ? m : OpmodeDefault());
                    shown.Text = px == null ? text : text + " (" + px + ")";
                }
                slider.ValueChanged += (_, __) =>
                {
                    var text = GameAppUi.SnapRange(row, slider.Value);
                    slider.Value = float.Parse(text, System.Globalization.CultureInfo.InvariantCulture);
                    Show();
                    if (!slider.Tracking) SetValue(row.Env, text); // VoiceOver adjusts without touches
                };
                EventHandler commit = (_, __) => SetValue(row.Env, GameAppUi.SnapRange(row, slider.Value));
                slider.TouchUpInside += commit;
                slider.TouchUpOutside += commit;
                if (row.Base != null) refreshers.Add(Show);
                Show();
                parts.Add(new UIStackView(new UIView[] { title, new UIView(), shown })
                {
                    Axis = UILayoutConstraintAxis.Horizontal,
                    Alignment = UIStackViewAlignment.Center,
                    Spacing = 8,
                });
                parts.Add(slider);
                break;
            }
            default: // choice
            {
                var options = row.Options;
                var index = -1;
                for (var i = 0; i < options.Count; i++) if (options[i].Value == current) index = i;
                if (options.Count <= 4)
                {
                    var control = new UISegmentedControl(options.Select(o => o.Label).ToArray()) { Enabled = enabled, AccessibilityIdentifier = id };
                    control.SelectedSegment = index < 0 ? -1 : index;
                    control.ValueChanged += (_, __) => SetValue(row.Env, options[(int) control.SelectedSegment].Value);
                    parts.Add(title);
                    parts.Add(control);
                }
                else
                {
                    var button = UIButton.FromType(UIButtonType.System);
                    button.Enabled = enabled;
                    button.AccessibilityIdentifier = id;
                    button.ShowsMenuAsPrimaryAction = true;
                    button.HorizontalAlignment = UIControlContentHorizontalAlignment.Trailing;
                    button.SetContentCompressionResistancePriority(250, UILayoutConstraintAxis.Horizontal);
                    string selected = index < 0 ? null : options[index].Value;
                    void Build()
                    {
                        var actions = new List<UIMenuElement>();
                        foreach (var (value, label) in options)
                        {
                            var v = value;
                            var action = UIAction.Create(label, null, null, _ =>
                            {
                                selected = v;
                                SetValue(row.Env, v);
                                Build();
                            });
                            if (v == selected) action.State = UIMenuElementState.On;
                            actions.Add(action);
                        }
                        button.Menu = UIMenu.Create("", actions.ToArray());
                        var chosen = options.Where(o => o.Value == selected).Select(o => o.Label).FirstOrDefault() ?? "Choose";
                        button.SetTitle(chosen, UIControlState.Normal);
                    }
                    Build();
                    parts.Add(new UIStackView(new UIView[] { title, new UIView(), button })
                    {
                        Axis = UILayoutConstraintAxis.Horizontal,
                        Alignment = UIStackViewAlignment.Center,
                        Spacing = 8,
                    });
                }
                break;
            }
        }
        if (row.Note.Length > 0) parts.Add(Text(row.Note, UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        if (row.Env == "DM_FPS_TARGET" && row.Kind == "choice" && row.Options.Any(o => o.Value == "120"))
            parts.Add(Text("120 fps is experimental: the game falls back to 60 when a frame is late, during movies and when the phone is hot.",
                           UIFontTextStyle.Footnote, UIColor.SecondaryLabel));
        return new UIStackView(parts.ToArray()) { Axis = UILayoutConstraintAxis.Vertical, Spacing = 6 };
    }

    // DM_OPMODE's default, for the pixel size of a render scale while the player has not chosen a mode.
    private string OpmodeDefault()
    {
        var row = launcher?.FirstOrDefault(r => r.Env == "DM_OPMODE");
        return row?.Default ?? "1";
    }
}


/// A game's icon straight from its dump: the control NCA's icon_<language>.dat (256x256 JPEG), read with the engine's
/// extractor's own functions, which are private to its class P (Program.cs, compiled into this assembly unchanged).
internal static class GameIcons
{
    [System.Diagnostics.CodeAnalysis.DynamicDependency(System.Diagnostics.CodeAnalysis.DynamicallyAccessedMemberTypes.NonPublicMethods, typeof(P))]
    internal static byte[] Read(string keysPath, string dumpPath)
    {
        const System.Reflection.BindingFlags Any = System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static;
        var open = typeof(P).GetMethod("OpenNsp", Any) ?? throw new MissingMethodException("P.OpenNsp");
        var pick = typeof(P).GetMethod("Pick", Any) ?? throw new MissingMethodException("P.Pick");
        var icon = typeof(P).GetMethod("Icon", Any) ?? throw new MissingMethodException("P.Icon");
        if (keysPath == null) return null;   // GameAppMain's self-check: the three are kept in the trimmed app
        var ks = LibHac.Common.Keys.ExternalKeyReader.ReadKeyFile(keysPath);
        // The enum types from the methods' own parameters: no guess at LibHac's namespaces.
        var metaType = Nullable.GetUnderlyingType(open.GetParameters()[2].ParameterType) ?? open.GetParameters()[2].ParameterType;
        var ncas = open.Invoke(null, new object[] { ks, dumpPath, Enum.Parse(metaType, "Application"), null, false });
        var ctrl = pick.Invoke(null, new object[] { ncas, Enum.Parse(pick.GetParameters()[1].ParameterType, "Control"), null });
        if (ctrl == null) return null;
        var result = ((byte[], string)) icon.Invoke(null, new object[] { ctrl });
        return result.Item1;
    }
}
