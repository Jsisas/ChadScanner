using Newtonsoft.Json;
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Linq;
using System.Windows.Forms;

namespace RatScanner;

/// <summary>
/// Everything about the most recent icon scan, kept in memory so a wrong result can be
/// saved after the fact with the hidden report hotkey (Ctrl+Shift+F12) and debugged offline.
/// </summary>
internal sealed class ScanReport {
	internal sealed record Candidate(string Id, string? ShortName, float Score);

	private static readonly object Gate = new();
	private static ScanReport? _last;

	public DateTimeOffset Time { get; } = DateTimeOffset.Now;
	public string Mouse { get; set; } = "";
	public string ScreenshotOrigin { get; set; } = "";
	public string Screen { get; } = $"{RatConfig.ScreenWidth}x{RatConfig.ScreenHeight} @ {RatConfig.ScreenScale}";
	public float RatEyeScale { get; set; }
	public bool UseChadVision { get; } = RatConfig.IconScan.UseChadVision;
	public bool ChadVisionLoaded { get; set; }

	public bool RatEyeFound { get; set; }
	public string? RatEyeIconPosition { get; set; }
	public string? RatEyeIconSize { get; set; }
	public string? RatEyeItemPosition { get; set; }
	public bool RatEyeRotated { get; set; }
	public float RatEyeConfidence { get; set; }
	public string? RatEyeItemId { get; set; }
	public string? RatEyeItemShortName { get; set; }

	/// <summary>"grid" when ChadVision found the cell from the stash grid lines, else RatEye's box was used.</summary>
	public string? CellSource { get; set; }
	/// <summary>x,y,w,h inside the screenshot.</summary>
	public string? CropRect { get; set; }
	public string? Slots { get; set; }
	public List<Candidate> Candidates { get; set; } = [];

	/// <summary>Which engine produced the shown item, and why the other one was not used.</summary>
	public string Outcome { get; set; } = "";
	public string? ShownItemId { get; set; }
	public string? ShownItemShortName { get; set; }

	[JsonIgnore] public Bitmap? Screenshot { get; set; }
	[JsonIgnore] public Bitmap? Crop { get; set; }

	/// <summary>Replaces the remembered scan; the previous one's bitmaps are released.</summary>
	internal static void Publish(ScanReport report) {
		lock (Gate) {
			_last?.Screenshot?.Dispose();
			_last?.Crop?.Dispose();
			_last = report;
		}
	}

	/// <summary>Writes the last scan to ScanReports/&lt;timestamp&gt;/. Returns the folder, or null if nothing was scanned yet.</summary>
	internal static string? SaveLast() {
		lock (Gate) {
			if (_last == null) return null;

			var dir = Path.Combine(RatConfig.Paths.Base, "ScanReports", _last.Time.ToString("yyyyMMdd-HHmmss-fff"));
			Directory.CreateDirectory(dir);

			_last.Screenshot?.Save(Path.Combine(dir, "screenshot.png"), ImageFormat.Png);
			_last.Crop?.Save(Path.Combine(dir, "crop.png"), ImageFormat.Png);
			File.WriteAllText(Path.Combine(dir, "report.json"), JsonConvert.SerializeObject(_last, Formatting.Indented));
			SaveScreen(Path.Combine(dir, "screen-at-report.png"));
			SaveLogTail(Path.Combine(dir, "log-tail.txt"));
			return dir;
		}
	}

	/// <summary>Whole virtual screen when the hotkey is pressed: shows what the overlay displayed.</summary>
	private static void SaveScreen(string path) {
		var bounds = SystemInformation.VirtualScreen;
		using var bmp = new Bitmap(bounds.Width, bounds.Height, PixelFormat.Format24bppRgb);
		using (var gfx = Graphics.FromImage(bmp)) gfx.CopyFromScreen(bounds.Left, bounds.Top, 0, 0, bounds.Size, CopyPixelOperation.SourceCopy);
		bmp.Save(path, ImageFormat.Png);
	}

	private static void SaveLogTail(string path) {
		if (!File.Exists(RatConfig.Paths.LogFile)) return;
		// The logger keeps the file open for writing.
		using var stream = new FileStream(RatConfig.Paths.LogFile, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
		using var reader = new StreamReader(stream);
		var lines = reader.ReadToEnd().Split('\n');
		File.WriteAllLines(path, lines.Skip(Math.Max(0, lines.Length - 300)));
	}
}
