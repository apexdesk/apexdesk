// ooxml-validate: validates Office Open XML files (.docx, .docm, .xlsx,
// .xlsm) with the Open XML SDK's OpenXmlValidator (the checks behind most
// "Word found unreadable content" repairs) and prints one JSON object per
// file, one per line:
//
//   {"file":"a.docx","ok":false,"errors":[{"id":"Sch_UnexpectedElementContentExpectingComplex",
//     "type":"Schema","part":"/word/document.xml","path":"/w:document[1]/w:body[1]/w:p[3]",
//     "element":"w:tblGrid","description":"..."}]}
//
// A file that doesn't open gets "open_error". Usage:
//
//   ooxml-validate [--version Office2007|...|Microsoft365] [--max N] FILE...
//   ooxml-validate ... @LIST   (file names, one per line)
//
// Build and install into target/tools: tools/ooxml-validate/setup.sh

using System.Text.Json;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Validation;

var files = new List<string>();
var version = FileFormatVersions.Microsoft365;
var max = 200;
for (var i = 0; i < args.Length; i++)
{
    switch (args[i])
    {
        case "--version":
            version = Enum.Parse<FileFormatVersions>(args[++i]);
            break;
        case "--max":
            max = int.Parse(args[++i]);
            break;
        default:
            if (args[i].StartsWith('@'))
                files.AddRange(File.ReadAllLines(args[i][1..]).Where(l => l.Trim().Length > 0));
            else
                files.Add(args[i]);
            break;
    }
}
if (files.Count == 0)
{
    Console.Error.WriteLine("usage: ooxml-validate [--version V] [--max N] FILE... | @LIST");
    return 2;
}

var validator = new OpenXmlValidator(version) { MaxNumberOfErrors = max };
var stdout = Console.OpenStandardOutput();
foreach (var file in files)
{
    using var buf = new MemoryStream();
    using (var w = new Utf8JsonWriter(buf))
    {
        w.WriteStartObject();
        w.WriteString("file", file);
        try
        {
            using OpenXmlPackage doc = Path.GetExtension(file).ToLowerInvariant() switch
            {
                ".xlsx" or ".xlsm" or ".xltx" or ".xltm" => SpreadsheetDocument.Open(file, false),
                ".pptx" or ".pptm" => PresentationDocument.Open(file, false),
                _ => WordprocessingDocument.Open(file, false),
            };
            var errors = validator.Validate(doc).ToList();
            w.WriteBoolean("ok", errors.Count == 0);
            w.WriteStartArray("errors");
            foreach (var e in errors)
            {
                w.WriteStartObject();
                w.WriteString("id", e.Id);
                w.WriteString("type", e.ErrorType.ToString());
                w.WriteString("part", e.Part?.Uri.ToString());
                w.WriteString("path", e.Path?.XPath);
                w.WriteString("element", e.Node is null ? null : $"{e.Node.Prefix}:{e.Node.LocalName}".TrimStart(':'));
                w.WriteString("description", e.Description);
                w.WriteEndObject();
            }
            w.WriteEndArray();
        }
        catch (Exception ex)
        {
            w.WriteBoolean("ok", false);
            w.WriteString("open_error", $"{ex.GetType().Name}: {ex.Message}");
        }
        w.WriteEndObject();
    }
    buf.WriteByte((byte)'\n');
    buf.Position = 0;
    buf.CopyTo(stdout);
    stdout.Flush();
}
return 0;
