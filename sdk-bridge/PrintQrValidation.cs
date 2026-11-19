using DYMO.LabelAPI;
using DYMO.LabelAPI.Interfaces;
using SkiaSharp;
using ZXing;

internal static class PrintQrValidation
{
    // Decode each object's own final raster. Reading the model's payload or a
    // freshly generated matrix cannot detect lost modules or obscuring objects.
    internal static string? Check(ILabel<ILayoutManager> model, byte[] preview)
    {
        var codes = model.LayoutManager.LabelObjects.OfType<QRCodeObject>().ToArray();
        if (codes.Length == 0) return null;
        using var bitmap = SKBitmap.Decode(preview);
        if (bitmap is null || model.LabelSize.Width <= 0 || model.LabelSize.Height <= 0)
            return "The printer-resolution QR preview could not be verified.";
        double xScale = bitmap.Width / (double)model.LabelSize.Width;
        double yScale = bitmap.Height / (double)model.LabelSize.Height;
        foreach (var code in codes)
        {
            var rect = code.Layout.Rect;
            int left = Math.Clamp((int)Math.Floor(rect.Location.X * xScale), 0, bitmap.Width);
            int top = Math.Clamp((int)Math.Floor(rect.Location.Y * yScale), 0, bitmap.Height);
            int right = Math.Clamp((int)Math.Ceiling((rect.Location.X + rect.Size.Width) * xScale), left, bitmap.Width);
            int bottom = Math.Clamp((int)Math.Ceiling((rect.Location.Y + rect.Size.Height) * yScale), top, bitmap.Height);
            // QR has at least 21 modules per side. A label object's height may
            // exceed its square symbol, so bound work by the shorter side.
            int size = Math.Min(right - left, bottom - top);
            if (size < 21 || !Matches(bitmap, left, top, right, bottom, code.Data.Text))
                return $"QR code '{code.Name}' cannot be read at printer resolution. Enlarge the code, shorten its contents or use wider tape.";
        }
        return null;
    }

    private static bool Matches(SKBitmap bitmap, int left, int top, int right, int bottom, string payload)
    {
        // Include actual surrounding tape so neighbouring ink and clipping
        // affect detection. Never invent a white border or repair missing dots.
        int margin = Math.Clamp(Math.Min(right - left, bottom - top) / 4, 4, 32);
        int sampleLeft = Math.Max(0, left - margin), sampleTop = Math.Max(0, top - margin);
        int width = Math.Min(bitmap.Width, right + margin) - sampleLeft;
        int height = Math.Min(bitmap.Height, bottom + margin) - sampleTop;
        if ((long)width * height > 1_000_000 || string.IsNullOrEmpty(payload)) return false;
        int scale = Math.Min(4, Math.Max(1, 1024 / Math.Max(width, height)));
        var pixels = new byte[width * scale * height * scale];
        var paper = bitmap.GetPixel(0, 0);
        for (int y = 0; y < height; y++)
        {
            for (int x = 0; x < width; x++)
            {
                var colour = bitmap.GetPixel(sampleLeft + x, sampleTop + y);
                int difference = Math.Max(Math.Abs(paper.Red - colour.Red),
                    Math.Max(Math.Abs(paper.Green - colour.Green), Math.Abs(paper.Blue - colour.Blue)));
                byte luminance = (byte)(255 - difference);
                Array.Fill(pixels, luminance, (y * width * scale + x) * scale, scale);
            }
            for (int row = 1; row < scale; row++)
                Array.Copy(pixels, y * width * scale * scale, pixels, (y * scale + row) * width * scale, width * scale);
        }
        var source = new RGBLuminanceSource(pixels, width * scale, height * scale, RGBLuminanceSource.BitmapFormat.Gray8);
        var reader = new BarcodeReaderGeneric();
        reader.Options.PossibleFormats = new[] { BarcodeFormat.QR_CODE };
        reader.Options.TryHarder = true;
        var result = reader.Decode(source);
        // Require finder patterns inside this object. A nearby duplicate with
        // the same contents cannot validate a missing or covered symbol.
        return result?.Text == payload && result.ResultPoints is { Length: >= 3 } points &&
            points.All(p => p.X >= (left - sampleLeft) * scale && p.X < (right - sampleLeft) * scale &&
                            p.Y >= (top - sampleTop) * scale && p.Y < (bottom - sampleTop) * scale);
    }
}
