// gameapp/GameAppNas.cs: a NAS reached over SFTP (SSH.NET, MIT), where exported .ipa files and save copies are kept.
//
// The settings (host, port, user, folder, the server's key fingerprint) are a small text file; the password is not in
// it: the app keeps it in its Keychain (GameAppMain). The server's key is trusted on first use and pinned: a server
// that later shows another key is refused with its fingerprint, as ssh does ("REMOTE HOST IDENTIFICATION HAS CHANGED").
// An upload goes to <name>.part, then is renamed: a cut connection never leaves a truncated file under the real name.
//
// No UIKit here.

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using Renci.SshNet;

public sealed record NasConfig(string Host, int Port, string User, string Folder, string HostKey)
{
    public static readonly NasConfig Empty = new NasConfig("", 22, "", "GameApp", "");

    public bool IsComplete => !string.IsNullOrWhiteSpace(Host) && !string.IsNullOrWhiteSpace(User) && Port > 0 && Port < 65536;

    public string Serialize() =>
        $"host\t{Host}\nport\t{Port}\nuser\t{User}\nfolder\t{Folder}\nhost_key\t{HostKey}\n";

    public static NasConfig Parse(string text)
    {
        var d = new Dictionary<string, string>();
        foreach (var raw in (text ?? "").Split('\n'))
        {
            var line = raw.TrimEnd('\r');
            var tab = line.IndexOf('\t');
            if (tab > 0) d[line.Substring(0, tab)] = line.Substring(tab + 1).Trim();
        }
        int port = d.TryGetValue("port", out var p) && int.TryParse(p, out var n) ? n : 22;
        return new NasConfig(d.GetValueOrDefault("host", ""), port, d.GetValueOrDefault("user", ""),
                             string.IsNullOrWhiteSpace(d.GetValueOrDefault("folder", "")) ? "GameApp" : d["folder"],
                             d.GetValueOrDefault("host_key", ""));
    }
}

public static class Nas
{
    /// The server key's fingerprint, as ssh prints it: SHA256:<base64 without padding>.
    public static string Fingerprint(byte[] hostKey) =>
        "SHA256:" + Convert.ToBase64String(SHA256.HashData(hostKey)).TrimEnd('=');

    /// The folder's parts, for creating it one level at a time ("a/b" -> "a", "a/b"); an absolute folder keeps its "/".
    public static List<string> FolderSteps(string folder)
    {
        var steps = new List<string>();
        var f = (folder ?? "").Replace('\\', '/').Trim();
        bool absolute = f.StartsWith("/");
        var parts = f.Split('/', StringSplitOptions.RemoveEmptyEntries);
        for (int i = 0; i < parts.Length; i++)
            steps.Add((absolute ? "/" : "") + string.Join("/", parts.Take(i + 1)));
        return steps;
    }

    private static SftpClient Connect(NasConfig cfg, AuthenticationMethod auth, Action<string> log, out string seenKey)
    {
        var info = new ConnectionInfo(cfg.Host, cfg.Port, cfg.User, auth) { Timeout = TimeSpan.FromSeconds(15) };
        var client = new SftpClient(info) { OperationTimeout = TimeSpan.FromMinutes(2) };
        string seen = null;
        client.HostKeyReceived += (_, e) =>
        {
            seen = Fingerprint(e.HostKey);
            // Trust on first use, then pinned.
            e.CanTrust = string.IsNullOrEmpty(cfg.HostKey) || cfg.HostKey == seen;
        };
        try { client.Connect(); }
        catch (Exception) when (seen != null && !string.IsNullOrEmpty(cfg.HostKey) && cfg.HostKey != seen)
        {
            client.Dispose();
            throw new InvalidOperationException(
                $"the NAS shows another key than the one saved ({seen}, saved {cfg.HostKey}): not connected. If the NAS was " +
                "reinstalled, forget its key in the NAS settings and connect again.");
        }
        seenKey = seen;
        log?.Invoke($"NAS: connected to {cfg.User}@{cfg.Host}:{cfg.Port} ({seen})");
        return client;
    }

    private static string EnsureFolder(SftpClient client, string folder)
    {
        foreach (var step in FolderSteps(folder))
            if (!client.Exists(step)) client.CreateDirectory(step);
        return string.IsNullOrWhiteSpace(folder) ? "." : folder.TrimEnd('/');
    }

    /// Connects, makes the folder, says how many files it holds. Returns the server key's fingerprint (to pin).
    public static string Test(NasConfig cfg, AuthenticationMethod auth, Action<string> log)
    {
        using var client = Connect(cfg, auth, log, out var seen);
        var dir = EnsureFolder(client, cfg.Folder);
        int n = client.ListDirectory(dir).Count(f => !f.IsDirectory);
        log?.Invoke($"NAS: folder {dir} ready ({n} file(s))");
        client.Disconnect();
        return seen;
    }

    /// Uploads `localPath` into the folder (subfolder optional), as <name>.part then renamed. Returns the server key's
    /// fingerprint. `log` gets a line per tenth of the bytes.
    public static string Upload(NasConfig cfg, AuthenticationMethod auth, string localPath, string subfolder, Action<string> log)
    {
        using var client = Connect(cfg, auth, log, out var seen);
        var dir = EnsureFolder(client, string.IsNullOrEmpty(subfolder) ? cfg.Folder : cfg.Folder.TrimEnd('/') + "/" + subfolder);
        var name = Path.GetFileName(localPath);
        var final = dir + "/" + name;
        var part = final + ".part";
        long total = new FileInfo(localPath).Length;
        int tenth = 0;
        var started = DateTime.Now;
        using (var input = File.OpenRead(localPath))
            client.UploadFile(input, part, true, sent =>
            {
                int t = total > 0 ? (int) ((long) sent * 10 / total) : 10;
                if (t > tenth) { tenth = t; log?.Invoke($"NAS: {t * 10} % of {name}"); }
            });
        if (client.Exists(final)) client.DeleteFile(final);
        client.RenameFile(part, final);
        var secs = Math.Max(0.1, (DateTime.Now - started).TotalSeconds);
        log?.Invoke($"NAS: {name} sent to {dir} ({total / 1048576.0:0.0} MB, {total / 1048576.0 / secs:0.0} MB/s)");
        client.Disconnect();
        return seen;
    }

    /// The folders inside `folder` (for choosing the sync folder): their full paths, sorted. "" is the login folder.
    public static List<string> Folders(NasConfig cfg, AuthenticationMethod auth, string folder, Action<string> log) =>
        Folders(cfg, auth, folder, log, out _);

    /// The same, and the server key's fingerprint (to pin at the first connection).
    public static List<string> Folders(NasConfig cfg, AuthenticationMethod auth, string folder, Action<string> log, out string seenKey)
    {
        using var client = Connect(cfg, auth, log, out seenKey);
        var dir = string.IsNullOrWhiteSpace(folder) ? "." : folder;
        var list = client.ListDirectory(dir).Where(f => f.IsDirectory && f.Name != "." && f.Name != ".." && !f.Name.StartsWith("."))
                         .Select(f => f.FullName).OrderBy(n => n, StringComparer.OrdinalIgnoreCase).ToList();
        client.Disconnect();
        return list;
    }

    /// Makes `path` (and the folders above it that are missing). Returns the path as the server names it.
    public static string MakeFolder(NasConfig cfg, AuthenticationMethod auth, string path, Action<string> log)
    {
        using var client = Connect(cfg, auth, log, out _);
        var made = EnsureFolder(client, path);
        client.Disconnect();
        log?.Invoke($"NAS: folder {made} ready");
        return made;
    }

    /// The games and .ipa files in `folder` (one level): CloudFile rows of `sourceId`, Remote the full path.
    public static List<CloudFile> List(NasConfig cfg, AuthenticationMethod auth, string folder, string sourceId, Action<string> log)
    {
        using var client = Connect(cfg, auth, log, out _);
        var dir = string.IsNullOrWhiteSpace(folder) ? "." : folder.TrimEnd('/');
        var files = client.ListDirectory(dir).Where(f => !f.IsDirectory && Cloud.Wanted(f.Name) && !f.Name.EndsWith(".part"))
                          .Select(f => new CloudFile(sourceId, f.Name, f.Length, f.FullName)).ToList();
        var exports = dir + "/Exports";   // where "Send to the NAS" puts exported apps
        if (client.Exists(exports))
            files.AddRange(client.ListDirectory(exports).Where(f => !f.IsDirectory && Cloud.IsIpa(f.Name))
                                 .Select(f => new CloudFile(sourceId, f.Name, f.Length, f.FullName)));
        client.Disconnect();
        log?.Invoke($"NAS: {files.Count} game or .ipa file(s) in {dir}");
        return files;
    }

    /// Downloads `remote` to `localPath` (as <localPath>.part, renamed at the end: a cut connection never leaves a file
    /// that looks whole). `progress` gets the bytes so far.
    public static void Download(NasConfig cfg, AuthenticationMethod auth, string remote, string localPath, Action<long, long> progress, Action<string> log)
    {
        using var client = Connect(cfg, auth, log, out _);
        long total = client.GetAttributes(remote).Size;
        var part = localPath + ".part";
        using (var output = File.Create(part))
            client.DownloadFile(remote, output, sent => progress?.Invoke((long) sent, total));
        if (File.Exists(localPath)) File.Delete(localPath);
        File.Move(part, localPath);
        client.Disconnect();
        log?.Invoke($"NAS: {Path.GetFileName(localPath)} downloaded ({total / 1048576.0:0.0} MB)");
    }

    /// Runs `read` on a seekable stream over `remote` (only the bytes it reads cross the network), then disconnects.
    public static T WithReadStream<T>(NasConfig cfg, AuthenticationMethod auth, string remote, Func<Stream, T> read)
    {
        using var client = Connect(cfg, auth, null, out _);
        using var stream = client.OpenRead(remote);
        try { return read(stream); }
        finally { client.Disconnect(); }
    }

    public static AuthenticationMethod Password(string user, string password) => new PasswordAuthenticationMethod(user, password ?? "");
}
