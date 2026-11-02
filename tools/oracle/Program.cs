// rf-oracle: the C# RedForge's file conversions, run headless, to check the Rust
// port against. Reads "input<TAB>output" lines from stdin and prints one
// "OK <input>" or "FAIL <input>: <error>" line for each.
//
//   rf-oracle toxml    loose game file (RSC8 with deflate, or binary) -> XML
//   rf-oracle fromxml  XML -> game file, as the C# writes it
//   rf-oracle floats   prints how .NET formats each float on stdin (one per line)

using System;
using System.Globalization;
using System.IO;
using RedForge.Core.Numerics;
using RedForge.Core.Utilities;
using RedForge.Games.RDR2;
using RedForge.Games.RDR2.RPF8;

static class Program
{
    static int Main(string[] args)
    {
        if (args.Length < 1)
        {
            Console.Error.WriteLine("usage: rf-oracle toxml|fromxml|floats < lines");
            return 2;
        }
        var mode = args[0];
        if (mode == "floats")
        {
            string l;
            while ((l = Console.In.ReadLine()) != null)
            {
                var bits = uint.Parse(l, NumberStyles.HexNumber);
                var f = BitConverter.UInt32BitsToSingle(bits);
                Console.WriteLine(FloatUtil.ToString(f));
            }
            return 0;
        }

        JenkIndex.LoadStringsFile("RDR2");
        var fm = new Rpf8FileManager(new RDR2Game());

        string line;
        while ((line = Console.In.ReadLine()) != null)
        {
            var parts = line.Split('\t');
            if (parts.Length < 2) continue;
            var input = parts[0];
            var output = parts[1];
            try
            {
                switch (mode)
                {
                    case "toxml":
                    {
                        var data = File.ReadAllBytes(input);
                        var name = Path.GetFileName(input);
                        var entry = fm.CreateFileEntry(name, input, ref data);
                        var folder = Path.GetDirectoryName(Path.GetFullPath(output));
                        var xml = fm.ConvertToXml(entry, data, out _, out _, folder);
                        if (string.IsNullOrEmpty(xml)) throw new Exception("no XML");
                        File.WriteAllText(output, xml);
                        break;
                    }
                    case "fromxml":
                    {
                        var xml = File.ReadAllText(input);
                        var name = Path.GetFileName(input);//foo.ytd.xml
                        var baseName = Path.GetFileNameWithoutExtension(Path.GetFileNameWithoutExtension(name));
                        var folder = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(input)), baseName);
                        var data = fm.ConvertFromXml(xml, name, folder);
                        if (data == null) throw new Exception("no data");
                        File.WriteAllBytes(output, data);
                        break;
                    }
                    default:
                        Console.Error.WriteLine("unknown mode " + mode);
                        return 2;
                }
                Console.WriteLine("OK " + input);
            }
            catch (Exception ex)
            {
                Console.WriteLine("FAIL " + input + ": " + ex.GetType().Name + ": " + ex.Message.Replace('\n', ' '));
            }
        }
        return 0;
    }
}
