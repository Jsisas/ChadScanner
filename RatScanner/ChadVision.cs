using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

namespace RatScanner;

/// <summary>
/// Wrapper around the native ChadVision library (chadvision.dll), which identifies
/// an item icon with DINOv2 embeddings and OCR.
/// </summary>
internal sealed partial class ChadVision : IDisposable {
	private const int MaxCandidates = 5;
	private const int IdLength = 32;

	// Must match the Rust `CvCandidate` exactly: 32 NUL-padded ASCII bytes, then an f32.
	[StructLayout(LayoutKind.Sequential, Pack = 1)]
	private unsafe struct CvCandidate {
		public fixed byte Id[IdLength];
		public float Score;
	}

	[LibraryImport("chadvision", EntryPoint = "cv_open", StringMarshalling = StringMarshalling.Utf8)]
	private static partial IntPtr CvOpen(string dataDir);

	[LibraryImport("chadvision", EntryPoint = "cv_identify")]
	private static unsafe partial int CvIdentify(IntPtr engine, byte* bgr, uint width, uint height, uint stride, uint slotW, uint slotH, CvCandidate* output, uint cap);

	[LibraryImport("chadvision", EntryPoint = "cv_close")]
	private static partial void CvClose(IntPtr engine);

	[LibraryImport("chadvision", EntryPoint = "cv_last_error")]
	private static unsafe partial uint CvLastError(byte* buffer, uint cap);

	// Must match the Rust `CvCell`.
	[StructLayout(LayoutKind.Sequential)]
	private struct CvCell {
		public int X, Y, W, H;
		public uint SlotW, SlotH;
	}

	[LibraryImport("chadvision", EntryPoint = "cv_locate_cell")]
	private static unsafe partial int CvLocateCell(byte* bgr, uint width, uint height, uint stride, int mouseX, int mouseY, float scale,
		uint hintSlotW, uint hintSlotH, CvCell* output);

	private readonly object _lock = new();
	private IntPtr _engine;

	private ChadVision(IntPtr engine) {
		_engine = engine;
	}

	/// <summary>
	/// Opens the engine from <paramref name="dataDir"/>, or returns <see langword="null"/>
	/// (and logs why) when the library or its data is unavailable.
	/// </summary>
	internal static ChadVision? TryOpen(string dataDir) {
		try {
			var engine = CvOpen(dataDir);
			if (engine != IntPtr.Zero) return new ChadVision(engine);
			Logger.LogWarning("ChadVision unavailable: " + GetLastError());
		} catch (Exception e) when (e is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException) {
			Logger.LogWarning("ChadVision unavailable: " + e.Message);
		}
		return null;
	}

	/// <summary>
	/// Identifies the item in <paramref name="crop"/>, best match first.
	/// </summary>
	/// <param name="crop">Screenshot region around the item icon</param>
	/// <param name="slotW">Width of the item in inventory slots</param>
	/// <param name="slotH">Height of the item in inventory slots</param>
	internal unsafe (string Id, float Score)[] Identify(Bitmap crop, int slotW, int slotH) {
		var candidates = new CvCandidate[MaxCandidates];
		int count;

		lock (_lock) {
			if (_engine == IntPtr.Zero) return [];

			var rect = new Rectangle(0, 0, crop.Width, crop.Height);
			var data = crop.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format24bppRgb);
			try {
				fixed (CvCandidate* output = candidates) {
					count = CvIdentify(_engine, (byte*)data.Scan0, (uint)data.Width, (uint)data.Height, (uint)data.Stride,
						(uint)slotW, (uint)slotH, output, (uint)candidates.Length);
				}
			} finally {
				crop.UnlockBits(data);
			}
		}

		if (count < 0) {
			Logger.LogWarning("ChadVision identify failed: " + GetLastError());
			return [];
		}

		var result = new (string Id, float Score)[Math.Min(count, candidates.Length)];
		for (var i = 0; i < result.Length; i++) {
			fixed (CvCandidate* candidate = &candidates[i]) {
				var length = 0;
				while (length < IdLength && candidate->Id[length] != 0) length++;
				result[i] = (Encoding.ASCII.GetString(candidate->Id, length), candidate->Score);
			}
		}
		return result;
	}

	/// <summary>
	/// Finds the item cell under <paramref name="mouse"/> from the stash grid lines in
	/// <paramref name="screenshot"/>. Hint slots fill in a side whose far edge is not visible.
	/// </summary>
	/// <returns>Cell rectangle in screenshot pixels and its size in slots, or <see langword="null"/> when no grid is visible there</returns>
	internal static unsafe (Rectangle Rect, int SlotW, int SlotH)? LocateCell(Bitmap screenshot, Point mouse, float scale, int hintSlotW, int hintSlotH) {
		CvCell cell;
		int found;
		var data = screenshot.LockBits(new Rectangle(0, 0, screenshot.Width, screenshot.Height), ImageLockMode.ReadOnly, PixelFormat.Format24bppRgb);
		try {
			found = CvLocateCell((byte*)data.Scan0, (uint)data.Width, (uint)data.Height, (uint)data.Stride, mouse.X, mouse.Y, scale,
				(uint)Math.Max(1, hintSlotW), (uint)Math.Max(1, hintSlotH), &cell);
		} finally {
			screenshot.UnlockBits(data);
		}

		if (found < 0) {
			Logger.LogWarning("ChadVision locate failed: " + GetLastError());
			return null;
		}
		return found == 0 ? null : (new Rectangle(cell.X, cell.Y, cell.W, cell.H), (int)cell.SlotW, (int)cell.SlotH);
	}

	private static unsafe string GetLastError() {
		var buffer = new byte[512];
		fixed (byte* p = buffer) {
			var written = CvLastError(p, (uint)buffer.Length);
			var length = (int)Math.Min(written, (uint)buffer.Length);
			while (length > 0 && buffer[length - 1] == 0) length--;
			return Encoding.UTF8.GetString(buffer, 0, length);
		}
	}

	public void Dispose() {
		lock (_lock) {
			if (_engine == IntPtr.Zero) return;
			CvClose(_engine);
			_engine = IntPtr.Zero;
		}
	}
}
