// gameapp/xlate/XlateCommonShim.cs -- the ONE hand-written stand-in for Ryujinx.Common in the in-app translator.
// Mirrors: dm/third_party/ryujinx/src/Ryujinx.Common/Utilities/EmbeddedResources.cs, ReadAllText(string) only. The real file needs
// StreamUtils -> Microsoft.IO.RecyclableMemoryStream (a NuGet package); the translator calls it from exactly one place
// (CodeGen/Glsl/Declarations.cs, the GLSL back end's helper functions), which the Spirv target never reaches.
using System.IO;
using System.Reflection;

namespace Ryujinx.Common
{
    public static class EmbeddedResources
    {
        public static string ReadAllText(string filename)
        {
            using Stream stream = typeof(EmbeddedResources).Assembly.GetManifestResourceStream(filename);
            if (stream == null) return null;
            using StreamReader reader = new(stream);
            return reader.ReadToEnd();
        }
    }
}
