// gameapp/xlate/GameAppXlate.cs -- the engine's Switch-shader translator (dmsh_xlate) run IN-PROCESS.
//
// Mirrors NOTHING: it calls the engine's own dm/gfx/tools/dmsh_xlate/Program.cs (class SwshXlate), compiled into the
// app unchanged from a copy, through its private Main(string[]) -- the exact entry the command-line tool has.
// Its stdout/stderr (the "translated: N ok, M failed" line, the "[RED] key: ..." lines) are captured PER CALLING
// THREAD: Console.Out / Console.Error are replaced once by a router that writes to the calling thread's capture while
// a Run is under way on it, and to the app's own console otherwise. So concurrent Run calls translate in parallel
// (dmsh_build.py hands each batch to J translator calls at once; the shim releases the GIL around each), where the
// first version swapped Console under one lock and the phone translated on one core (docs/DECISIONS.md trap 42).
// The translator itself keeps no state between calls (no static field in Program.cs; Ryujinx's Translator is used by
// Ryujinx from several threads, one context per shader). Output files, text and exit code are the CLI's: 0 only if
// every job translated.
using System;
using System.Diagnostics.CodeAnalysis;
using System.IO;
using System.Reflection;
using System.Text;

internal static class GameAppXlate
{
    static MethodInfo _main;
    static readonly object InstallGate = new object();
    static bool _installed;

    [ThreadStatic] static StringWriter t_out;
    [ThreadStatic] static StringWriter t_err;

    // Writes to this thread's capture when it has one, else to the writer Console had before.
    sealed class Router : TextWriter
    {
        readonly TextWriter _fallback;
        readonly bool _err;
        public Router(TextWriter fallback, bool err) { _fallback = fallback; _err = err; }
        TextWriter Target => (_err ? t_err : t_out) ?? _fallback;
        public override Encoding Encoding => _fallback.Encoding;
        public override void Write(char value) => Target.Write(value);
        public override void Write(string value) => Target.Write(value);
        public override void Write(char[] buffer, int index, int count) => Target.Write(buffer, index, count);
        public override void WriteLine(string value) => Target.WriteLine(value);
        public override void WriteLine() => Target.WriteLine();
        public override void Flush() => Target.Flush();
    }

    static void Install()
    {
        lock (InstallGate)
        {
            if (_installed) return;
            // Console.SetOut wraps a writer in TextWriter.Synchronized: each write takes a lock for one call, which is
            // what keeps the app's own console lines whole; a thread's capture is its own and never contended.
            Console.SetOut(new Router(Console.Out, false));
            Console.SetError(new Router(Console.Error, true));
            _installed = true;
        }
    }

    // Main is private in the engine's file and the app is trimmed: keep it, and find it by reflection.
    [DynamicDependency("Main(System.String[])", typeof(SwshXlate))]
    static MethodInfo Main()
        => _main ??= typeof(SwshXlate).GetMethod("Main", BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.Static, null, new[] { typeof(string[]) }, null)
                     ?? throw new MissingMethodException("SwshXlate.Main(string[])");

    static int Invoke(string[] args, out string stdout, out string stderr)
    {
        Install();
        var so = new StringWriter(); var se = new StringWriter();
        StringWriter oldOut = t_out, oldErr = t_err;   // a nested call (none today) keeps its caller's capture
        t_out = so; t_err = se;
        int rc;
        try { rc = (int)Main().Invoke(null, new object[] { args }); }
        catch (TargetInvocationException e)   // what an unhandled exception would have printed in the CLI
        {
            se.WriteLine("Unhandled exception. " + (e.InnerException ?? e));
            rc = 134;
        }
        finally { t_out = oldOut; t_err = oldErr; }
        stdout = so.ToString(); stderr = se.ToString();
        return rc;
    }

    /// <summary>dmsh_xlate &lt;jobJsonPath&gt; &lt;outDir&gt;: writes outDir/&lt;key&gt;.spv, .res.json (+ .reader.spv); 0 only if all ok.</summary>
    public static int Run(string jobJsonPath, string outDir, out string stdout, out string stderr)
        => Invoke(new[] { jobJsonPath, outDir }, out stdout, out stderr);

    /// <summary>dmsh_xlate --source-sha: first 16 hex of sha256(Program.cs + dmsh_xlate.csproj as the engine has them).</summary>
    public static string SourceSha()
    {
        Invoke(new[] { "--source-sha" }, out string so, out _);
        return so.Trim();
    }
}
