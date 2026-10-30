using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

namespace RatScanner;

/// <summary>
/// Wrapper around the native tarkov-vision library (tv.dll), which identifies
/// an item icon with DINOv2 embeddings and OCR.
/// </summary>
internal sealed partial class TarkovVision : IDisposable {
	private const int MaxCandidates = 5;
	private const int IdLength = 32;

	// Must match the Rust `TvCandidate` exactly: 32 NUL-padded ASCII bytes, then an f32.
	[StructLayout(LayoutKind.Sequential, Pack = 1)]
	private unsafe struct TvCandidate {
		public fixed byte Id[IdLength];
		public float Score;
	}

	[LibraryImport("tv", EntryPoint = "tv_open", StringMarshalling = StringMarshalling.Utf8)]
	private static partial IntPtr TvOpen(string dataDir);

	[LibraryImport("tv", EntryPoint = "tv_identify")]
	private static unsafe partial int TvIdentify(IntPtr engine, byte* bgr, uint width, uint height, uint stride, uint slotW, uint slotH, TvCandidate* output, uint cap);

	[LibraryImport("tv", EntryPoint = "tv_close")]
	private static partial void TvClose(IntPtr engine);

	[LibraryImport("tv", EntryPoint = "tv_last_error")]
	private static unsafe partial uint TvLastError(byte* buffer, uint cap);

	private readonly object _lock = new();
	private IntPtr _engine;

	private TarkovVision(IntPtr engine) {
		_engine = engine;
	}

	/// <summary>
	/// Opens the engine from <paramref name="dataDir"/>, or returns <see langword="null"/>
	/// (and logs why) when the library or its data is unavailable.
	/// </summary>
	internal static TarkovVision? TryOpen(string dataDir) {
		try {
			var engine = TvOpen(dataDir);
			if (engine != IntPtr.Zero) return new TarkovVision(engine);
			Logger.LogWarning("tarkov-vision unavailable: " + GetLastError());
		} catch (Exception e) when (e is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException) {
			Logger.LogWarning("tarkov-vision unavailable: " + e.Message);
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
		var candidates = new TvCandidate[MaxCandidates];
		int count;

		lock (_lock) {
			if (_engine == IntPtr.Zero) return [];

			var rect = new Rectangle(0, 0, crop.Width, crop.Height);
			var data = crop.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format24bppRgb);
			try {
				fixed (TvCandidate* output = candidates) {
					count = TvIdentify(_engine, (byte*)data.Scan0, (uint)data.Width, (uint)data.Height, (uint)data.Stride,
						(uint)slotW, (uint)slotH, output, (uint)candidates.Length);
				}
			} finally {
				crop.UnlockBits(data);
			}
		}

		if (count < 0) {
			Logger.LogWarning("tarkov-vision identify failed: " + GetLastError());
			return [];
		}

		var result = new (string Id, float Score)[Math.Min(count, candidates.Length)];
		for (var i = 0; i < result.Length; i++) {
			fixed (TvCandidate* candidate = &candidates[i]) {
				var length = 0;
				while (length < IdLength && candidate->Id[length] != 0) length++;
				result[i] = (Encoding.ASCII.GetString(candidate->Id, length), candidate->Score);
			}
		}
		return result;
	}

	private static unsafe string GetLastError() {
		var buffer = new byte[512];
		fixed (byte* p = buffer) {
			var written = TvLastError(p, (uint)buffer.Length);
			var length = (int)Math.Min(written, (uint)buffer.Length);
			while (length > 0 && buffer[length - 1] == 0) length--;
			return Encoding.UTF8.GetString(buffer, 0, length);
		}
	}

	public void Dispose() {
		lock (_lock) {
			if (_engine == IntPtr.Zero) return;
			TvClose(_engine);
			_engine = IntPtr.Zero;
		}
	}
}
