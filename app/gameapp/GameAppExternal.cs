// gameapp/GameAppExternal.cs -- a TV or monitor (USB-C DisplayPort, AirPlay) shows the game; the iPhone is the controller
// (docs/DECISIONS.md entry 91).
//
// How the picture gets there. The engine presents into ONE CAMetalLayer (front_metal_layer, handed over by
// gameapp_set_game_layer) and reads that layer afresh for every frame: nextDrawable and drawableSize at each present
// (nvn_mtl.m present_now), and the MetalFX spatial scaler of the present (DM_PRESENT_UPSCALE=metalfx) is rebuilt by itself
// when the output size it is asked for changes (fx_encode). Nothing is cached, so the layer need not be replaced: its view
// (GameAppGameView) is moved from the phone's view controller to a window on the external display's UIWindowScene, and
// the view's own LayoutSubviews sizes the drawable to the pixels it has there. No host change for the move.
//
// What iOS decides, and what this file does about it:
//   * An external display is a UIWindowScene of role UIWindowSceneSessionRoleExternalDisplayNonInteractive. iOS mirrors the
//     phone on it until the app has a window there: the engine's own front end says so (front_ios.m DmTVSceneDelegate:
//     "without a window iOS goes on mirroring the phone"). The mirror setting therefore just makes no window; the scene
//     stays connected and the phone is mirrored as it always was.
//   * iOS 27 offers the scene only to an app that registers a scene accessory (UIViewController.registerSceneAccessory with
//     UISceneAccessory.externalNonInteractive); the Info.plist entry alone is the iOS 26 path. Neither exists in the iOS 26
//     bindings this app is built with, so both go through the Objective-C runtime. Registered while a game is on screen,
//     and only when the player wants the game on the TV: with the accessory unregistered iOS mirrors, so a menu or a game
//     set to "Mirror" never has a scene offered at all.
//   * A display connected while the game runs, one connected before it starts and one removed again are the same code:
//     SceneConnected / SceneDisconnected move the picture, and the game goes on (nothing is stopped or rebuilt).
//
// The drawable. The external display's own pixels (UIScreen.NativeBounds * scale), up to 3840 across with MetalFX
// upscaling on (the scaler's output is the drawable: that is its job), and 1920 across without it: the engine measured a 4K
// drawable on AirPlay as black and the GPU going from 15 to 60 ms a frame (front_ios.m tv_attach), and a plain stretch
// from 1080p costs the compositor nothing. The cap counts the PICTURE's width, not the display's, so a display of another
// shape still gets all its pixels across the picture.
//
// The phone. The host's touch gamepad over the whole screen, no picture, dark ground (load/front_tv.m:
// dm_front_pad_layout_tv, the engine's own DmLayout.onTV layout); the console mode goes to docked while the TV shows the
// picture. With a hardware controller connected the engine's pad hides its controls AND its menu button in that layout, so a
// "Playing on the TV" cover with its own menu button (the host's dm_game_menu) takes the screen. Portrait works too.
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;

using CoreAnimation;
using CoreGraphics;
using Foundation;
using Metal;
using GameController;
using ObjCRuntime;
using UIKit;

public static class GameAppExternal
{
    [DllImport("libc", EntryPoint = "getenv")] private static extern IntPtr c_getenv(string name);
    [DllImport("/usr/lib/libobjc.dylib", EntryPoint = "objc_msgSend")] private static extern IntPtr msg_ptr(IntPtr receiver, IntPtr selector, IntPtr a);
    [DllImport("/usr/lib/libobjc.dylib", EntryPoint = "objc_msgSend")] private static extern void msg_void_ptr(IntPtr receiver, IntPtr selector, IntPtr a);

    /// "tv" (default): the picture goes to the display and the phone is the controller; "mirror": iOS mirrors the phone.
    public const string SettingEnv = "DM_EXTERNAL_DISPLAY";

    private static UIWindowScene scene;            // the connected external display's scene (null: none, or the debug stand-in)
    private static UIWindow window;                // the picture's window, only while it is there
    private static GameAppGameViewController owner;   // the game on screen
    private static NSObject registration;          // iOS 27: the scene accessory's registration
    private static bool wantTv;
    private static UIWindowScene debugScene;       // -GameAppDebugTv: the phone's own scene stands in for a display
    public static Action<string> Log = _ => { };

    public static string Env(string name)
    {
        var p = c_getenv(name);
        return p == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(p);
    }

    /// The player's choice for this game: the switch the pipeline put in the environment, else the game's settings.tsv,
    /// else the default (the game on the TV).
    public static bool PlayerWantsTv(string settingsFile)
    {
        var v = Env(SettingEnv);
        if (string.IsNullOrEmpty(v) && settingsFile != null && File.Exists(settingsFile))
        {
            try { GameAppUi.ParseSettingValues(File.ReadAllText(settingsFile)).TryGetValue(SettingEnv, out v); }
            catch (IOException) { v = null; }
        }
        return !string.Equals(v, "mirror", StringComparison.OrdinalIgnoreCase);
    }

    public static bool MetalFxOn => string.Equals(Env("DM_PRESENT_UPSCALE"), "metalfx", StringComparison.OrdinalIgnoreCase);

    /// The game's window is on screen. Registers the scene offer (iOS 27) and, when a display is already connected, moves the
    /// picture to it.
    public static void Begin(GameAppGameViewController game, bool tv)
    {
        owner = game;
        wantTv = tv;
        if (!tv) { Log("external display: set to Mirror, the phone stays the screen (iOS mirrors it on a display)"); return; }
        Register(game);
        if (debugScene != null) { SceneConnected(debugScene, true); return; }
        if (scene != null) Attach();
    }

    /// The game's window is gone: the picture is back on the phone (nothing to move: the view goes with its controller).
    public static void End(GameAppGameViewController game)
    {
        if (owner != game) return;
        Detach(true);
        Unregister(game);
        owner = null;
    }

    public static void SceneConnected(UIWindowScene s, bool debug = false)
    {
        if (!debug) scene = s;
        Log($"external display connected ({s.Screen.NativeBounds.Width:0}x{s.Screen.NativeBounds.Height:0} px, scale {s.Screen.NativeScale:0.##})");
        if (owner != null && wantTv) Attach(s);
    }

    public static void SceneDisconnected(UIWindowScene s)
    {
        if (scene != s) return;
        Log("external display disconnected: the picture is back on the phone");
        Detach(false);
        scene = null;
    }

    private static void Attach(UIWindowScene target = null)
    {
        target ??= scene;
        if (owner == null || target == null || window != null) return;
        var tv = new GameAppTvViewController(owner);
        UIWindow w;
        if (target == debugScene)
        {
            // Stand-in for a display: a 16:9 window over the phone's top right, so one screen shows both ends of the move
            var b = target.CoordinateSpace.Bounds;
            w = new UIWindow(target) { Frame = new CGRect(b.Width - 330, 24, 320, 180), WindowLevel = UIWindowLevel.Alert + 10 };
        }
        else
        {
            w = new UIWindow(target);
        }
        w.RootViewController = tv;
        w.Hidden = false;   // without a window iOS goes on mirroring; not key, so the phone's window keeps the keyboard
        window = w;
        owner.MovePictureTo(tv.View, w.Screen, MaxPixels());
        GameAppHost.TvAttached(true);
        Log("external display: the game picture is on the display, the phone is the controller");
    }

    private static void Detach(bool gameEnding)
    {
        if (window == null) return;
        var w = window;
        window = null;
        if (!gameEnding) owner?.MovePictureBack();
        GameAppHost.TvAttached(false);
        w.Hidden = true;
        w.RootViewController = null;
    }

    private static double MaxPixels() => MetalFxOn ? 3840 : 1920;

    public static bool OnDisplay => window != null;

    // ---- the scene offer (iOS 27's accessory) ---------------------------------------------------------------------

    private static void Register(UIViewController vc)
    {
        if (registration != null || !vc.RespondsToSelector(new Selector("registerSceneAccessory:"))) return;
        try
        {
            var config = new UISceneConfiguration("ExternalDisplay", UIWindowSceneSessionRole.ExternalDisplayNonInteractive) { DelegateType = typeof(GameAppExternalSceneDelegate) };
            var accessory = msg_ptr(new Class("UISceneAccessory").Handle, Selector.GetHandle("externalNonInteractiveSceneAccessoryWithConfiguration:"), config.Handle);
            var reg = msg_ptr(vc.Handle, Selector.GetHandle("registerSceneAccessory:"), accessory);
            if (reg != IntPtr.Zero) registration = Runtime.GetNSObject(reg);
            Log(registration != null ? "external display: scene accessory registered (iOS 27)" : "external display: iOS refused the scene accessory");
        }
        catch (Exception e) { Log($"external display: the scene accessory could not be registered ({e.GetType().Name}: {e.Message})"); }
    }

    private static void Unregister(UIViewController vc)
    {
        if (registration == null) return;
        try { msg_void_ptr(vc.Handle, Selector.GetHandle("unregisterSceneAccessory:"), registration.Handle); }
        catch (Exception) { /* the controller is going away with its registration */ }
        registration = null;
    }

    // ---- the debug stand-in (simulator: Simulator.app's External Displays needs a human) ---------------------------

    /// -GameAppDebugTv: the phone's own scene plays the external display, so the whole flow (the move, the drawable's size,
    /// the controller layout, the way back) runs where no display can be attached from the command line.
    public static void UseDebugDisplay(UIWindowScene phone) { debugScene = phone; }
    public static void DebugToggle()
    {
        if (debugScene == null) return;
        if (window != null) SceneDisconnected2(); else if (owner != null) SceneConnected(debugScene, true);
    }
    private static void SceneDisconnected2()
    {
        Log("external display disconnected (debug stand-in): the picture is back on the phone");
        Detach(false);
    }
}

/// The scene delegate of the external display: it only remembers the scene. The window is made by GameAppExternal.Attach when a
/// game wants the picture there; with no window iOS mirrors the phone, which is what a menu and "Mirror" want.
[Register("GameAppExternalSceneDelegate")]
public class GameAppExternalSceneDelegate : UIResponder, IUIWindowSceneDelegate
{
    [Export("window")] public UIWindow Window { get; set; }

    [Export("scene:willConnectToSession:options:")]
    public void WillConnect(UIScene scene, UISceneSession session, UISceneConnectionOptions options)
        => GameAppExternal.SceneConnected((UIWindowScene) scene);

    [Export("sceneDidDisconnect:")]
    public void DidDisconnect(UIScene scene) => GameAppExternal.SceneDisconnected((UIWindowScene) scene);
}

/// The external display's root view controller: black, with the game view in it at the picture's shape.
public sealed class GameAppTvViewController : UIViewController
{
    private readonly GameAppGameViewController game;
    public GameAppTvViewController(GameAppGameViewController game) { this.game = game; }

    public override void LoadView() => View = new UIView { BackgroundColor = UIColor.Black, ClipsToBounds = true };

    public override void ViewDidLayoutSubviews()
    {
        base.ViewDidLayoutSubviews();
        game.LayoutPictureOnDisplay(View.Bounds);
    }

    public override bool PrefersStatusBarHidden() => true;
}

/// The host's entries for the TV (load/front_tv.m, through the shim: a host built before them answers "no").
public static class GameAppHost
{
    [DllImport("__Internal")] private static extern int gameapp_tv_attached(int on);
    [DllImport("__Internal")] private static extern int gameapp_pad_layout_tv(IntPtr pad, double width, double height);
    [DllImport("__Internal")] private static extern IntPtr gameapp_game_menu();

    public static void TvAttached(bool on) { try { gameapp_tv_attached(on ? 1 : 0); } catch (EntryPointNotFoundException) { } }
    public static bool PadLayoutTv(IntPtr pad, double w, double h)
    {
        try { return gameapp_pad_layout_tv(pad, w, h) != 0; } catch (EntryPointNotFoundException) { return false; }
    }
    /// The host's in-game menu (the gear's: controller layout, Show FPS, game settings, Quit game), or null.
    public static UIMenu GameMenu()
    {
        try
        {
            var h = gameapp_game_menu();
            return h == IntPtr.Zero ? null : Runtime.GetNSObject<UIMenu>(h, true);
        }
        catch (EntryPointNotFoundException) { return null; }
    }
}

/// -GameAppDebugGame: the game window with a test Metal layer and no game, so the scene switching, the drawable's size and
/// the controller layout can be seen on a simulator (no key, no dump). The layer is cleared to a colour that shifts with
/// time, so a frozen present shows; the drawable's size and the
/// screen it is on are written to Documents/debug-external.log once a second. With -GameAppDebugTv the phone's own scene
/// plays the external display and a button toggles it.
public static class GameAppDebugGame
{
    private static CADisplayLink link;
    private static IMTLCommandQueue queue;
    private static int frame;
    private static string logPath;

    public static void Show(UIViewController root)
    {
        var vc = new GameAppGameViewController();
        logPath = Path.Combine(GameAppMain.Documents, "debug-external.log");
        File.WriteAllText(logPath, "");
        GameAppExternal.Log = text => { try { File.AppendAllText(logPath, text + "\n"); } catch (IOException) { } Console.WriteLine("[external] " + text); };
        var layer = vc.MetalLayerForDebug;
        queue = layer.Device.CreateCommandQueue();
        var top = root;
        while (top.PresentedViewController != null) top = top.PresentedViewController;
        top.PresentViewController(vc, false, () =>
        {
            var button = UIButton.FromType(UIButtonType.System);
            button.SetTitle("debug: toggle TV", UIControlState.Normal);
            button.BackgroundColor = UIColor.FromWhiteAlpha(1f, 0.85f);
            button.Layer.CornerRadius = 10;
            button.ContentEdgeInsets = new UIEdgeInsets(6, 10, 6, 10);
            button.AccessibilityIdentifier = "debug-toggle-tv";
            button.Frame = new CGRect(16, vc.View.Bounds.Height - 60, 150, 36);
            button.AutoresizingMask = UIViewAutoresizing.FlexibleTopMargin;
            button.AddTarget((_, _) => { GameAppExternal.DebugToggle(); vc.View.BringSubviewToFront(button); }, UIControlEvent.TouchUpInside);
            vc.View.AddSubview(button);
        });
        link = CADisplayLink.Create(() => Draw(layer, vc));
        link.PreferredFramesPerSecond = 30;
        link.AddToRunLoop(NSRunLoop.Main, NSRunLoopMode.Common);
    }

    private static void Draw(CAMetalLayer layer, GameAppGameViewController vc)
    {
        frame++;
        var size = layer.DrawableSize;
        if (size.Width < 1 || size.Height < 1) return;
        using var drawable = layer.NextDrawable();
        if (drawable == null) return;
        var t = frame / 60.0;
        var pass = new MTLRenderPassDescriptor();
        pass.ColorAttachments[0].Texture = drawable.Texture;
        pass.ColorAttachments[0].LoadAction = MTLLoadAction.Clear;
        pass.ColorAttachments[0].StoreAction = MTLStoreAction.Store;
        pass.ColorAttachments[0].ClearColor = new MTLClearColor(0.15 + 0.1 * Math.Sin(t), 0.35, 0.55 + 0.2 * Math.Cos(t), 1);
        using var cb = queue.CommandBuffer();
        using var enc = cb.CreateRenderCommandEncoder(pass);
        enc.EndEncoding();
        cb.PresentDrawable(drawable);
        cb.Commit();
        if (frame % 30 == 0)
        {
            var screen = vc.GameScreenForDebug;
            GameAppExternal.Log($"layer drawable {size.Width:0}x{size.Height:0} contentsScale {layer.ContentsScale:0.##} on {(screen == null ? "no screen" : $"{screen.Bounds.Width:0}x{screen.Bounds.Height:0} pt")}; picture on display: {GameAppExternal.OnDisplay}");
        }
    }
}
