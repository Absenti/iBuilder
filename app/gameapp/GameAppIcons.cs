// gameapp/GameAppIcons.cs: a game's icon read from a STREAM -- a dump on the NAS (an SFTP stream, random access) or in
// a Files folder -- so a game only in the cloud shows its cover before it is downloaded. Only the package's header,
// its tickets and its small control NCA are read (a few hundred KB), never the game. The same steps as the extractor's
// own OpenNsp / Pick / Icon (engine dm/tools/nx_extract/Program.cs), on a storage over the stream; Icon itself is the
// extractor's, through reflection as GameIcons does. NSP, NSZ (its control NCA is a plain .nca) and XCI.
using System;
using System.IO;
using LibHac.Common;
using LibHac.Common.Keys;
using LibHac.Fs;
using LibHac.Fs.Fsa;
using LibHac.FsSystem;
using LibHac.Spl;
using LibHac.Tools.Fs;
using LibHac.Tools.FsSystem;
using LibHac.Tools.FsSystem.NcaUtils;

internal static class GameIconsStream
{
    [System.Diagnostics.CodeAnalysis.DynamicDependency(System.Diagnostics.CodeAnalysis.DynamicallyAccessedMemberTypes.NonPublicMethods, typeof(P))]
    internal static byte[] Read(string keysPath, Stream stream)
    {
        const System.Reflection.BindingFlags Any = System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static;
        var icon = typeof(P).GetMethod("Icon", Any) ?? throw new MissingMethodException("P.Icon");
        var ks = ExternalKeyReader.ReadKeyFile(keysPath);
        IStorage st0 = new StreamStorage(stream, true);
        st0.GetSize(out long size).ThrowIfFailure();
        var magic = new byte[4];
        if (size >= 0x104) st0.Read(0x100, magic).ThrowIfFailure();
        IFileSystem pfs;
        if (System.Text.Encoding.ASCII.GetString(magic) == "HEAD")   // XCI: the card header's magic at 0x100
        {
            var xci = new Xci(ks, st0);
            if (!xci.HasPartition(XciPartitionType.Secure)) return null;
            pfs = xci.OpenPartition(XciPartitionType.Secure);
        }
        else
        {
            var p = new PartitionFileSystem();
            p.Initialize(st0).ThrowIfFailure();
            pfs = p;
        }
        Nca control = null;
        foreach (DirectoryEntryEx e in pfs.EnumerateEntries("*", SearchOptions.Default))   // tickets first in name order? not guaranteed: two passes
        {
            if (!e.Name.EndsWith(".tik", StringComparison.OrdinalIgnoreCase)) continue;
            using var f = new UniqueRef<IFile>();
            pfs.OpenFile(ref f.Ref, e.FullPath.ToU8Span(), OpenMode.Read).ThrowIfFailure();
            var t = new byte[0x2c0];
            f.Get.Read(out _, 0, t, ReadOption.None).ThrowIfFailure();
            if (t[0x281] != 0) continue;   // a personalised ticket: no title key here
            ks.ExternalKeySet.Add(new RightsId(t.AsSpan(0x2a0, 0x10)), new AccessKey(t.AsSpan(0x180, 0x10))).ThrowIfFailure();
        }
        foreach (DirectoryEntryEx e in pfs.EnumerateEntries("*", SearchOptions.Default))
        {
            if (!e.Name.EndsWith(".nca", StringComparison.OrdinalIgnoreCase) || e.Size > 64L << 20) continue;   // the control NCA is small
            using var f = new UniqueRef<IFile>();
            pfs.OpenFile(ref f.Ref, e.FullPath.ToU8Span(), OpenMode.Read).ThrowIfFailure();
            var nca = new Nca(ks, f.Release().AsStorage());
            if (nca.Header.ContentType == NcaContentType.Control) { control = nca; break; }
        }
        if (control == null) return null;
        var result = ((byte[], string)) icon.Invoke(null, new object[] { control });
        return result.Item1;
    }
}
