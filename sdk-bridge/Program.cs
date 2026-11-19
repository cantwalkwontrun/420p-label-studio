using System.Text.Json;
using System.IO;
using System.Xml.Linq;
using System.Runtime.InteropServices;
using System.Text;
using DymoSDK.Implementations;
using DymoSDK.Interfaces;

// One JSON request on stdin and one JSON result on stdout. All label contents stay local.
try
{
    var request = JsonSerializer.Deserialize<Request>(Console.In.ReadToEnd(), new JsonSerializerOptions { PropertyNameCaseInsensitive = true }) ?? throw new InvalidOperationException("Missing request");
    if (request.Action == "fonts")
    {
        var fonts = System.Windows.Media.Fonts.SystemFontFamilies.Select(f => f.Source).Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(x => x, StringComparer.OrdinalIgnoreCase).ToArray();
        Console.WriteLine(JsonSerializer.Serialize(new { ok = true, fonts }));
        return;
    }
    if (request.Action == "usb-presence")
    {
        Console.WriteLine(JsonSerializer.Serialize(new { ok = true, usbPresent = DevicePresence.Is420pPresent() }));
        return;
    }
    if (request.Action == "parse-device-status")
    {
        if (request.StatusBase64 is not { Length: <= 64 }) throw new InvalidOperationException("Supply a bounded synthetic status reply.");
        Console.WriteLine(JsonSerializer.Serialize(DeviceStatus.Decode(Convert.FromBase64String(request.StatusBase64)), new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase }));
        return;
    }
    if (request.Action == "device-status")
    {
        var status = DeviceStatus.Disconnected();
        if (DevicePresence.Is420pPresent())
        {
            try
            {
                DymoSDK.App.Init();
                status = DeviceStatus.Read();
            }
            catch (Exception)
            {
                status = new DeviceStatusResult(true, false, Message: "Printer details are unavailable. Try refreshing the connection.");
            }
        }
        Console.WriteLine(JsonSerializer.Serialize(status, new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase }));
        return;
    }
    DymoSDK.App.Init();
    if (request.Action == "import-label") { Console.WriteLine(JsonSerializer.Serialize(NativeLabelImport.Read(request.TemplateBase64, request.IncludeDesign))); return; }
    if (request.Action == "printers")
    {
        var printers = DymoPrinter.Instance.GetPrinters().Where(p => p.Name == "DYMO LabelManager 420P").Select(p => new { p.Name, p.IsConnected }).ToArray();
        Console.WriteLine(JsonSerializer.Serialize(new { ok = true, printers, usbPresent = DevicePresence.Is420pPresent() }));
        return;
    }
    if (request.Action != "preview" && request.Action != "print" && request.Action != "inspect-layout" && request.Action != "export") throw new InvalidOperationException("Unsupported action");
    if (request.Action == "inspect-layout" && request.Layout is not null) throw new InvalidOperationException("Layout inspection cannot submit a manual layout.");
    if (request.Width is not (6 or 9 or 12 or 19)) throw new InvalidOperationException("Select a valid tape width");
    XDocument doc;
    // A rich-text run or a data-table cell can consist entirely of spaces.
    const LoadOptions loadOptions = LoadOptions.PreserveWhitespace;
    if (request.TemplateBase64 is not null)
    {
        if (request.TemplateBase64.Length == 0 || request.TemplateBase64.Length > NativeLabelImport.MaximumBase64Length) throw new InvalidOperationException("Template snapshot is empty or too large");
        var templateBytes = Convert.FromBase64String(request.TemplateBase64);
        if (templateBytes.Length == 0 || templateBytes.Length > NativeLabelImport.MaximumBytes) throw new InvalidOperationException("Template snapshot is empty or too large");
        using var templateStream = new MemoryStream(templateBytes, false);
        doc = XDocument.Load(templateStream, loadOptions);
    }
    else
    {
        if (string.IsNullOrWhiteSpace(request.Template) || !File.Exists(request.Template)) throw new InvalidOperationException("Template not found");
        doc = XDocument.Load(request.Template, loadOptions);
    }
    XElement? sourceTable = null;
    if (request.Action == "export" && doc.Root is { } sourceRoot)
    {
        var tables = sourceRoot.Elements("DataTable").Take(2).ToArray();
        if (tables.Length > 1) throw new InvalidOperationException("Multiple embedded data tables cannot be exported safely. Keep this document as a .420p project.");
        sourceTable = tables.FirstOrDefault();
        // Keep this outer Connect document data separate from label-object edits.
        sourceTable?.Remove();
    }
    // Without xml:space, the SDK discards a whitespace-only text run on load.
    static bool IsSpaceRun(XElement run) => run.Value.Length > 0 && string.IsNullOrWhiteSpace(run.Value);
    static bool MarkSpaceRuns(XDocument xml)
    {
        var spaces = xml.Descendants("LabelObjects").Elements().SelectMany(o => o.Descendants("TextSpan").Elements("Text"))
            .Where(IsSpaceRun).ToArray();
        foreach (var span in spaces) span.SetAttributeValue(XNamespace.Xml + "space", "preserve");
        return spaces.Length > 0;
    }
    var containsSpaceRuns = MarkSpaceRuns(doc);
    var name = doc.Descendants("LabelName").FirstOrDefault()?.Value ?? throw new InvalidOperationException("Template tape width is missing");
    if (!name.StartsWith($"{request.Width}MM", StringComparison.OrdinalIgnoreCase) && !name.StartsWith($"{request.Width}X", StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException($"Template tape does not match the selected {request.Width} mm tape");
    // Frozen XML does not freeze values evaluated later by the SDK. Embedded
    // live dates can change between preview and print, while embedded counters
    // bypass the client's per-label sequence planning. Reject these template
    // objects before any render or print; objects added below use the app's
    // captured date values and explicit counter batches instead.
    if (doc.Descendants("DateTimeObject").Any(date => !string.Equals(date.Element("IsStatic")?.Value.Trim(), "True", StringComparison.OrdinalIgnoreCase)))
        throw new InvalidOperationException("This template contains a dynamic date/time object. Make it static in DYMO software, or remove it and add date/time in this app before printing.");
    if (doc.Descendants("CounterObject").Any())
        throw new InvalidOperationException("This template contains an embedded counter. Remove it and add a counter in this app so every label can be previewed and checked before printing.");
    Dictionary<string, LayoutResolved>? originalNativeLayout = null;
    if (request.NativeTemplate && (request.Layout is not null || request.Action == "preview") &&
        doc.Descendants("LabelObjects").Elements().Skip(1).Any())
    {
        var originalLabel = DymoLabel.LabelSharedInstance;
        originalLabel.LoadLabelFromXML(doc.ToString(SaveOptions.DisableFormatting));
        try { originalNativeLayout = InteractiveLayout.Snapshot(originalLabel).ToDictionary(o => o.Name, StringComparer.OrdinalIgnoreCase); }
        catch (Exception) when (request.Layout is null) { /* Optional for display-only previews. */ }
        // Empty text boxes can overlap artwork without carrying any printed content.
        // Typing into one later must still validate the new collision.
        foreach (var blank in doc.Descendants("TextObject").Where(o => o.Descendants("Text").All(t => string.IsNullOrWhiteSpace(t.Value))))
            if (blank.Element("Name") is { } blankName) originalNativeLayout?.Remove(blankName.Value);
    }
    if (request.Text != null && !doc.Descendants("TextObject").Any())
    {
        var labelObjects = doc.Descendants("LabelObjects").FirstOrDefault() ?? throw new InvalidOperationException("Template cannot contain label objects");
        var text = XElement.Load(Path.Combine(AppContext.BaseDirectory, "default-text.xml"));
        var height = doc.Descendants("DYMORect").FirstOrDefault()?.Element("Size")?.Element("Height")?.Value;
        var objectHeight = text.Element("ObjectLayout")?.Element("Size")?.Element("Height");
        if (height == null || objectHeight == null) throw new InvalidOperationException("Template dimensions are missing");
        objectHeight.Value = height;
        labelObjects.Add(text);
    }
    if (!string.IsNullOrEmpty(request.Image) && !doc.Descendants("ImageObject").Any())
    {
        var labelObjects = doc.Descendants("LabelObjects").FirstOrDefault() ?? throw new InvalidOperationException("Template cannot contain label objects");
        var imageObject = XElement.Load(Path.Combine(AppContext.BaseDirectory, "default-image.xml"));
        var data = imageObject.Element("Data") ?? throw new InvalidOperationException("Image template data is missing");
        data.Value = request.Image;
        var height = doc.Descendants("DYMORect").FirstOrDefault()?.Element("Size")?.Element("Height")?.Value ?? throw new InvalidOperationException("Template dimensions are missing");
        var size = imageObject.Element("ObjectLayout")?.Element("Size") ?? throw new InvalidOperationException("Image dimensions are missing");
        size.Element("Width")!.Value = height;
        size.Element("Height")!.Value = height;
        var position = doc.Descendants("DYMORect").FirstOrDefault()?.Element("DYMOPoint") ?? throw new InvalidOperationException("Template position is missing");
        var x = double.Parse(position.Element("X")!.Value, System.Globalization.CultureInfo.InvariantCulture);
        var h = double.Parse(height, System.Globalization.CultureInfo.InvariantCulture);
        var textX = doc.Descendants("TextObject").FirstOrDefault()?.Element("ObjectLayout")?.Element("DYMOPoint")?.Element("X");
        if (textX != null) textX.Value = (x + h + 0.04).ToString(System.Globalization.CultureInfo.InvariantCulture);
        labelObjects.AddFirst(imageObject);
    }
    var editableTextObjects = (request.NativeTemplate ? doc.Descendants("TextObject").Take(1) : doc.Descendants("TextObject")).ToArray();
    if (!request.PreserveTemplateFormatting)
    {
        if (string.IsNullOrWhiteSpace(request.Font) || request.Font.Length > 128 || request.Font.Any(char.IsControl)) throw new InvalidOperationException("Invalid font");
        if (request.Align is not ("Left" or "Center" or "Right")) throw new InvalidOperationException("Invalid text alignment");
        foreach (var text in editableTextObjects)
        {
            var fonts = text.Descendants("FontInfo").ToArray();
            var original = fonts.FirstOrDefault();
            void ApplyFontProperty(string property, string value, string fallback)
            {
                if (request.NativeTemplate && string.Equals(original?.Element(property)?.Value ?? fallback, value, StringComparison.OrdinalIgnoreCase)) return;
                foreach (var font in fonts) font.Element(property)?.SetValue(value);
            }
            ApplyFontProperty("IsBold", request.Bold.ToString(), "False");
            ApplyFontProperty("IsItalic", request.Italic.ToString(), "False");
            ApplyFontProperty("IsUnderline", request.Underline.ToString(), "False");
            ApplyFontProperty("FontName", request.Font, "Segoe UI");
            if (!request.NativeTemplate)
                foreach (var colour in text.Descendants("FontBrush").Descendants("Color"))
                {
                    colour.SetAttributeValue("A", "1"); colour.SetAttributeValue("R", "0"); colour.SetAttributeValue("G", "0"); colour.SetAttributeValue("B", "0");
                }
            if (!request.NativeTemplate || (text.Element("HorizontalAlignment")?.Value ?? "Center") != request.Align)
                foreach (var alignment in text.DescendantsAndSelf("HorizontalAlignment")) alignment.SetValue(request.Align);
        }
    }
    var label = DymoLabel.LabelSharedInstance;
    label.LoadLabelFromXML(doc.ToString(SaveOptions.DisableFormatting));
    var objects = label.GetLabelObjects().ToList();
    string? emptyPrimaryText = null;
    if (request.Text != null)
    {
        var textObject = objects.FirstOrDefault(o => o.Type == TypeObject.TextObject) ?? throw new InvalidOperationException("Template has no text object");
        if (!label.UpdateLabelObject(textObject, request.Text)) throw new InvalidOperationException("Unable to update template text");
        if (string.IsNullOrWhiteSpace(request.Text)) emptyPrimaryText = textObject.Name;
    }
    else if (request.NativeTemplate && editableTextObjects.FirstOrDefault() is { } primary &&
        primary.Descendants("Text").All(t => string.IsNullOrWhiteSpace(t.Value)))
    {
        // An unchanged native label sends no replacement text. Its empty editor
        // placeholder must still be excluded from hit testing and collisions.
        emptyPrimaryText = primary.Element("Name")?.Value;
    }
    if (!string.IsNullOrEmpty(request.Image))
    {
        var imageObject = objects.FirstOrDefault(o => o.Type == TypeObject.ImageObject) ?? throw new InvalidOperationException("Template has no image object");
        if (!label.SetImageFromBase64(imageObject.Name, request.Image)) throw new InvalidOperationException("Unable to update template image");
    }
    var featureObjects = request.Objects ?? Array.Empty<FeatureObject>();
    if (featureObjects.Any(o => o is not null && string.Equals(o.Kind, "date", StringComparison.OrdinalIgnoreCase) && string.IsNullOrWhiteSpace(o.Value)))
        throw new InvalidOperationException("Date objects need a fixed date so the print matches the preview.");
    if (request.Action == "print" && request.Copies > 1 && featureObjects.Any(o => o is not null && string.Equals(o.Kind, "counter", StringComparison.OrdinalIgnoreCase)))
        throw new InvalidOperationException("Sequential counter printing has not been verified; print one label at a time.");
    var objectLayouts = FeatureObjects.Add(label, featureObjects, deferOverlapValidation: request.Layout is not null || request.Action == "inspect-layout", emptyPrimaryText: emptyPrimaryText).ToArray();
    if (request.Action == "inspect-layout")
    {
        // Inspection never prints. Its geometry is used only to merge object
        // identities; the resulting full layout must pass Apply before print.
        var resolved = InteractiveLayout.Snapshot(label);
        var field = typeof(DymoLabel).GetField("_instanceLabel", System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
        if (field?.GetValue(label) is not DYMO.LabelAPI.Interfaces.ILabel<DYMO.LabelAPI.Interfaces.ILayoutManager> model)
            throw new InvalidOperationException("SDK printable bounds are unavailable.");
        var bounds = model.LayoutBounds.Size;
        var canExpand = !model.HasFixedLength && model.LayoutManager.LengthBehavior == DYMO.LabelAPI.ELabelLengthBehavior.VariableLength;
        Console.WriteLine(JsonSerializer.Serialize(new { ok = true, allLayouts = resolved, printableLengthMm = (double)bounds.Width * 25.4, printableWidthMm = (double)bounds.Height * 25.4, isFixedLength = !canExpand }));
        return;
    }
    var manualAdjustedObjects = new List<string>();
    if (request.Layout is not null)
    {
        // Deferred overlap checking is safe only because Apply validates every
        // object and fails closed before this request can submit a print job.
        var resolved = InteractiveLayout.Apply(label, request.Layout, emptyPrimaryText, originalNativeLayout);
        foreach (var item in resolved)
        {
            var requested = request.Layout.First(o => string.Equals(o.Name, item.Name, StringComparison.OrdinalIgnoreCase));
            if (Math.Abs(item.Xmm - requested.Xmm) > 0.15 || Math.Abs(item.Ymm - requested.Ymm) > 0.15 ||
                Math.Abs(item.WidthMm - requested.WidthMm) > 0.15 || Math.Abs(item.HeightMm - requested.HeightMm) > 0.15)
                manualAdjustedObjects.Add(item.Name);
        }
        objectLayouts = featureObjects.Select(spec => {
            var item = resolved.First(o => string.Equals(o.Name, spec.Name, StringComparison.OrdinalIgnoreCase));
            var adjusted = manualAdjustedObjects.Contains(item.Name);
            return new FeatureObjectLayout(item.Name, spec.Kind, item.Xmm, item.Ymm, item.WidthMm, item.HeightMm, adjusted);
        }).ToArray();
    }
    // LabelName alone is not evidence of the template's actual tape geometry.
    // Keep this optional with the existing private geometry lookup, but reject
    // a clearly wrong internal height whenever the SDK exposes it.
    var geometryField = typeof(DymoLabel).GetField("_instanceLabel", System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
    string? printIssue = null;
    if (geometryField?.GetValue(label) is DYMO.LabelAPI.Interfaces.ILabel<DYMO.LabelAPI.Interfaces.ILayoutManager> geometry)
    {
        label.GetPreviewLabel();
        printIssue = PrintQrValidation.Check(geometry, label.Preview);
        var actualTapeWidthMm = (double)geometry.LabelSize.Height * 25.4;
        if (double.IsFinite(actualTapeWidthMm) && actualTapeWidthMm > 0 && Math.Abs(actualTapeWidthMm - request.Width) > 2.0)
            throw new InvalidOperationException($"Template tape geometry ({actualTapeWidthMm:F1} mm) does not match the selected {request.Width} mm tape. Choose a matching template.");
    }
    else if (doc.Descendants("QRCodeObject").Any() || featureObjects.Any(o => string.Equals(o.Kind, "qr", StringComparison.OrdinalIgnoreCase)))
        printIssue = "The printer-resolution QR geometry is unavailable.";
    if (printIssue is not null && request.Action is "print" or "export")
        throw new InvalidOperationException(printIssue);
    if (request.Action == "export")
    {
        if (label.GetPreviewLabel() is not { Length: > 0 } expectedPreview) throw new InvalidOperationException("The label cannot be rendered for export.");
        // Save alone can retain the original XML and omit model edits. Clone
        // the resolved model first, and require the exact printer-resolution
        // bitmap before saving. Fail closed if SDK internals change.
        var serializer = typeof(DymoLabel).GetMethod("SerializeLabel", System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
        if (serializer?.Invoke(label, null) is not string resolvedXml || resolvedXml.Length > NativeLabelImport.MaximumBytes)
            throw new InvalidOperationException("This SDK cannot export the resolved label safely.");
        var sourceLabel = label as DymoLabel ?? throw new InvalidOperationException("This SDK cannot create an independent export label.");
        var exportLabel = sourceLabel.GetNewLabelInstance();
        if (ReferenceEquals(label, exportLabel)) throw new InvalidOperationException("The SDK could not create an independent export label.");
        var exportXml = resolvedXml;
        if (containsSpaceRuns)
        {
            var resolved = XDocument.Parse(resolvedXml, LoadOptions.PreserveWhitespace);
            MarkSpaceRuns(resolved);
            exportXml = resolved.ToString(SaveOptions.DisableFormatting);
        }
        exportLabel.LoadLabelFromXML(exportXml);
        if (!exportLabel.GetPreviewLabel().AsSpan().SequenceEqual(expectedPreview))
            throw new InvalidOperationException("The exported label would differ from the verified preview.");
        // Save uses the SDK's public API. Never accept a caller-supplied file
        // path; return the frozen label bytes for the Rust client to save.
        var directory = Path.Combine(Path.GetTempPath(), "dymo420p-export-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, "label.dymo");
        try
        {
            if (!exportLabel.Save(path)) throw new InvalidOperationException("DYMO could not export the label.");
            var size = new FileInfo(path).Length;
            if (size is <= 0 or > NativeLabelImport.MaximumBytes) throw new InvalidOperationException("The exported label is empty or larger than 16 MiB.");
            if (sourceTable is not null || containsSpaceRuns)
            {
                var saved = XDocument.Load(path, LoadOptions.PreserveWhitespace);
                if (saved.Root?.Name != "DesktopLabel") throw new InvalidOperationException("The SDK did not create a desktop label.");
                var changedText = request.Text is null ? null : editableTextObjects.FirstOrDefault();
                var originalTexts = containsSpaceRuns ? doc.Descendants("LabelObjects").Elements()
                    .Where(o => !ReferenceEquals(o, changedText) && o.Descendants("TextSpan").Elements("Text").Any(IsSpaceRun)).ToArray() : [];
                static XElement[] MatchingRuns(XDocument document, XElement original)
                {
                    var text = document.Descendants("LabelObjects").Elements(original.Name)
                        .SingleOrDefault(t => t.Element("Name")?.Value == original.Element("Name")?.Value);
                    return text?.Descendants("TextSpan").Elements("Text").ToArray() ?? [];
                }
                foreach (var original in originalTexts)
                {
                    var sourceRuns = original.Descendants("TextSpan").Elements("Text").ToArray();
                    var savedRuns = MatchingRuns(saved, original);
                    if (sourceRuns.Length != savedRuns.Length) throw new InvalidOperationException("The SDK changed the original text runs. Keep this file as a .420p project.");
                    for (int i = 0; i < sourceRuns.Length; ++i)
                        if (string.IsNullOrWhiteSpace(savedRuns[i].Value) && string.IsNullOrWhiteSpace(sourceRuns[i].Value))
                            savedRuns[i].Value = sourceRuns[i].Value;
                }
                if (containsSpaceRuns) MarkSpaceRuns(saved);
                if (sourceTable is not null)
                {
                    var outputTable = saved.Root.Elements("DataTable").SingleOrDefault();
                    if (outputTable is null) saved.Root.Add(new XElement(sourceTable));
                    else outputTable.ReplaceWith(new XElement(sourceTable));
                }
                saved.Save(path, SaveOptions.DisableFormatting);
                if (new FileInfo(path).Length > NativeLabelImport.MaximumBytes) throw new InvalidOperationException("The exported label exceeds 16 MiB.");
                var verified = XDocument.Load(path, LoadOptions.PreserveWhitespace);
                if (sourceTable is not null && !XNode.DeepEquals(sourceTable, verified.Root?.Element("DataTable")))
                    throw new InvalidOperationException("The saved data table differed from the source.");
                foreach (var original in originalTexts)
                    if (!original.Descendants("TextSpan").Elements("Text").Select(t => t.Value)
                        .SequenceEqual(MatchingRuns(verified, original).Select(t => t.Value)))
                        throw new InvalidOperationException("The SDK could not preserve the original text runs. Keep this file as a .420p project.");
            }
            var bytes = File.ReadAllBytes(path);
            var restored = sourceLabel.GetNewLabelInstance();
            if (ReferenceEquals(label, restored) || ReferenceEquals(exportLabel, restored)) throw new InvalidOperationException("The SDK could not verify the exported label independently.");
            restored.LoadLabelFromFilePath(path);
            if (!restored.GetPreviewLabel().AsSpan().SequenceEqual(expectedPreview) ||
                serializer.Invoke(restored, null) is not string restoredXml ||
                !XNode.DeepEquals(XDocument.Parse(resolvedXml), XDocument.Parse(restoredXml)))
                throw new InvalidOperationException("The exported label did not preserve the verified content and layout.");
            Console.WriteLine(JsonSerializer.Serialize(new { ok = true, labelBase64 = Convert.ToBase64String(bytes) }));
        }
        finally
        {
            // Only these two agent-created paths are touched, never recursively.
            try { File.Delete(path); } finally { Directory.Delete(directory); }
        }
        return;
    }
    if (request.Action == "preview")
    {
        label.GetPreviewLabel();
        // DYMO does not expose the resolved, auto-sized label dimensions on its public SDK
        // interface. Treat this private model as optional and fail closed if it changes.
        double? lengthMm = null, canvasWidthMm = null, printableLengthMm = null, printableWidthMm = null, printableXmm = null, printableYmm = null;
        try
        {
            var field = typeof(DymoLabel).GetField("_instanceLabel", System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            if (field?.GetValue(label) is DYMO.LabelAPI.Interfaces.ILabel<DYMO.LabelAPI.Interfaces.ILayoutManager> model)
            {
                var size = model.LabelSize;
                var length = (double)size.Width * 25.4;
                var width = (double)size.Height * 25.4;
                if (double.IsFinite(length) && double.IsFinite(width) && length > 0 && length <= 10000 && width > 0 && width <= 100)
                {
                    lengthMm = length; canvasWidthMm = width;
                }
                var bounds = model.LayoutBounds.Size;
                var printableLength = (double)bounds.Width * 25.4;
                var printableWidth = (double)bounds.Height * 25.4;
                var left = (double)model.LayoutBounds.Location.X * 25.4;
                var top = (double)model.LayoutBounds.Location.Y * 25.4;
                if (double.IsFinite(printableLength) && double.IsFinite(printableWidth) && printableLength > 0 && printableLength <= 10000 && printableWidth > 0 && printableWidth <= 100)
                { printableLengthMm = printableLength; printableWidthMm = printableWidth; }
                if (double.IsFinite(left) && double.IsFinite(top) && left >= 0 && top >= 0 && left + printableLength <= length + 0.15 && top + printableWidth <= width + 0.15)
                { printableXmm = left; printableYmm = top; }
            }
        }
        catch (Exception) { /* The preview remains usable if SDK internals change. */ }
        IReadOnlyList<LayoutResolved>? allLayouts = null;
        try { allLayouts = InteractiveLayout.Snapshot(label); } catch (Exception) { /* Optional for existing labels; manual layout remains unavailable. */ }
        if (allLayouts is not null && originalNativeLayout is not null)
            allLayouts = allLayouts.Select(row => row with { NativeBounds = originalNativeLayout.GetValueOrDefault(row.Name) }).ToArray();
        var standardPreview = label.Preview;
        var designPreview = request.IncludeDesign ? HighResolutionPreview.TryRender(label, standardPreview) : null;
        if (designPreview is not null && ((long)standardPreview.Length + designPreview.Length) * 4 / 3 > 30_000_000) designPreview = null;
        Console.WriteLine(JsonSerializer.Serialize(new { ok = true, preview = Convert.ToBase64String(standardPreview), designPreview = designPreview is null ? null : Convert.ToBase64String(designPreview), lengthMm, canvasWidthMm, printableLengthMm, printableWidthMm, printableXmm, printableYmm, printIssue, objects = objects.Select(o => o.Type.ToString()).ToArray(), objectLayouts, allLayouts, manualAdjustedObjects, emptyPrimaryText }));
        return;
    }
    if (request.Copies is < 1 or > 100) throw new InvalidOperationException("Copies must be between 1 and 100");
    // Recheck final rendered ink here as well as in the editor. An image path
    // alone cannot establish that a thresholded image produced a printable dot.
    using (var raster = SkiaSharp.SKBitmap.Decode(label.GetPreviewLabel()) ?? throw new InvalidOperationException("The print image is unavailable."))
    {
        var paper = raster.GetPixel(0, 0);
        if (!raster.Pixels.Any(pixel => pixel.Alpha != 0 && pixel != paper))
            throw new InvalidOperationException("The preview contains no printable ink.");
    }
    if (!DevicePresence.Is420pPresent()) throw new InvalidOperationException("The 420P USB device is not connected");
    var printer = DymoPrinter.Instance.GetPrinters().FirstOrDefault(p => p.Name == "DYMO LabelManager 420P" && p.IsConnected) ?? throw new InvalidOperationException("The DYMO LabelManager 420P is not connected");
    var currentStatus = DeviceStatus.ReadConnected();
    if (currentStatus.PrintBlock is not null) throw new InvalidOperationException(string.Join(" ", currentStatus.Issues));
    var printed = await DymoPrinter.Instance.PrintLabel(label, printer.Name, request.Copies);
    if (!printed) throw new InvalidOperationException("DYMO did not accept the print job");
    Console.WriteLine(JsonSerializer.Serialize(new { ok = true }));
}
catch (Exception e)
{
    Console.Error.WriteLine(JsonSerializer.Serialize(new { ok = false, error = e.Message }));
    Environment.ExitCode = 1;
}

internal sealed record Request(string Action, string? Template, string? Text, int Copies = 1, string? Image = null, int Width = 0, bool Bold = false, bool Italic = false, bool Underline = false, string Font = "Segoe UI", string Align = "Center", FeatureObject[]? Objects = null, string? TemplateBase64 = null, bool IncludeDesign = true, LayoutSpec[]? Layout = null, string? StatusBase64 = null, bool NativeTemplate = false, bool PreserveTemplateFormatting = false);

internal static class DevicePresence
{
    [StructLayout(LayoutKind.Sequential)]
    private struct DeviceInfo { public int Size; public Guid ClassGuid; public uint DeviceInstance; public IntPtr Reserved; }
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr SetupDiGetClassDevsW(IntPtr guid, string? enumerator, IntPtr hwnd, uint flags);
    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern bool SetupDiEnumDeviceInfo(IntPtr set, uint index, ref DeviceInfo data);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SetupDiGetDeviceInstanceIdW(IntPtr set, ref DeviceInfo data, StringBuilder id, int size, out int required);
    [DllImport("setupapi.dll")]
    private static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);

    internal static bool Is420pPresent()
    {
        var set = SetupDiGetClassDevsW(IntPtr.Zero, "USB", IntPtr.Zero, 0x02 | 0x04);
        if (set == new IntPtr(-1)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            for (uint i = 0; ; i++)
            {
                var data = new DeviceInfo { Size = Marshal.SizeOf<DeviceInfo>() };
                if (!SetupDiEnumDeviceInfo(set, i, ref data))
                {
                    var error = Marshal.GetLastWin32Error();
                    if (error == 259) return false;
                    throw new System.ComponentModel.Win32Exception(error);
                }
                var id = new StringBuilder(512);
                if (!SetupDiGetDeviceInstanceIdW(set, ref data, id, id.Capacity, out _)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
                var value = id.ToString();
                if (value.StartsWith("USB\\VID_0922&PID_1004\\", StringComparison.OrdinalIgnoreCase)) return true;
            }
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
    }
}
