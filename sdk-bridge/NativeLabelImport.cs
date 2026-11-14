using System.IO;
using System.Text;
using System.Xml;
using System.Xml.Linq;
using DymoSDK.Implementations;

internal static class NativeLabelImport
{
    internal static object Read(string? encoded, bool includeReference)
    {
        if (encoded is null || encoded.Length is < 1 or > 5_592_408) throw new InvalidOperationException("The DYMO label is empty or larger than 4 MiB.");
        var bytes = Convert.FromBase64String(encoded);
        if (bytes.Length is < 1 or > 4_194_304) throw new InvalidOperationException("The DYMO label is empty or larger than 4 MiB.");
        using var stream = new MemoryStream(bytes, false);
        using var reader = XmlReader.Create(stream, new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null, MaxCharactersInDocument = 8_388_608 });
        var doc = XDocument.Load(reader, LoadOptions.PreserveWhitespace);
        if (doc.Root?.Name.LocalName != "DesktopLabel") throw new InvalidOperationException("This label format is not a DYMO Connect tape label supported by the 420P. Open it in DYMO Connect and save it as .dymo first.");
        var name = doc.Descendants("LabelName").FirstOrDefault()?.Value ?? "";
        var width = new[] { 6, 9, 12, 19 }.FirstOrDefault(w => name.StartsWith($"{w}MM", StringComparison.OrdinalIgnoreCase) || name.StartsWith($"{w}X", StringComparison.OrdinalIgnoreCase));
        if (width == 0) throw new InvalidOperationException("The 420P requires a 6, 9, 12 or 19 mm D1 tape label.");
        var objects = doc.Descendants("LabelObjects").FirstOrDefault()?.Elements().ToArray() ?? throw new InvalidOperationException("The label's object list is missing.");
        if (objects.Length > 64) throw new InvalidOperationException("A label can contain at most 64 objects.");
        var first = objects.FirstOrDefault(e => e.Name.LocalName == "TextObject");
        var lines = first?.Descendants("LineTextSpan").Select(line => string.Concat(line.Descendants("Text").Select(e => e.Value))).ToArray() ?? Array.Empty<string>();
        var text = string.Join("\n", lines);
        if (text.EnumerateRunes().Count() > 500) throw new InvalidOperationException("The first text object exceeds this editor's 500-character limit.");
        var font = first?.Descendants("FontInfo").FirstOrDefault();
        string Font(string key, string fallback) => font?.Element(key)?.Value ?? fallback;
        // Validate with the vendor reader, but never print or save the source.
        var label = DymoLabel.LabelSharedInstance;
        label.LoadLabelFromXML(doc.ToString(SaveOptions.DisableFormatting));
        var preview = label.GetPreviewLabel();
        if (preview is not { Length: > 0 }) throw new InvalidOperationException("DYMO could not render this label.");
        return new { ok = true, width, text, font = Font("FontName", "Segoe UI"), bold = Font("IsBold", "False").Equals("True", StringComparison.OrdinalIgnoreCase),
            italic = Font("IsItalic", "False").Equals("True", StringComparison.OrdinalIgnoreCase), underline = Font("IsUnderline", "False").Equals("True", StringComparison.OrdinalIgnoreCase),
            align = first?.Element("HorizontalAlignment")?.Value ?? "Center", referencePreview = includeReference ? Convert.ToBase64String(preview) : null };
    }
}
