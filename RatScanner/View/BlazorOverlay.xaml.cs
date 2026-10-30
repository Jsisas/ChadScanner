using Microsoft.Extensions.DependencyInjection;
using Microsoft.Web.WebView2.Core;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Forms;
using System.Windows.Interop;

namespace RatScanner.View;

/// <summary>
/// Interaction logic for BlazorOverlay.xaml
/// </summary>
public partial class BlazorOverlay : Window {
	private bool _shown;
	private int _version;

	public BlazorOverlay(ServiceProvider serviceProvider) {
		Resources.Add("services", serviceProvider);

		InitializeComponent();
	}

	internal async void SetVisible(bool visible) {
		var version = ++_version;
		if (visible) {
			if (_shown) return;
			_shown = true;
			SetSize();
			Show();
			return;
		}
		if (!_shown) return;
		// A hidden WebView2 stops compositing and re-presents its last frame when shown again,
		// so only hide once the empty overlay has actually been painted (two animation frames).
		try {
			var webView = blazorOverlayWebView.WebView;
			if (webView.CoreWebView2 != null) {
				_ = await webView.ExecuteScriptAsync(
					"new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(0))))");
			}
		} catch (System.Exception e) when (e is System.InvalidOperationException or COMException) {
			// WebView torn down at exit: hide anyway.
		}
		if (version != _version) return; // a newer show/hide arrived while waiting
		_shown = false;
		Hide();
	}
	private void BlazorOverlay_Loaded(object? sender, RoutedEventArgs e) {
		blazorOverlayWebView.WebView.DefaultBackgroundColor = System.Drawing.Color.Transparent;
		SetSize();
		SetWindowStyle();
		blazorOverlayWebView.WebView.NavigationCompleted += WebView_Loaded;
		blazorOverlayWebView.WebView.CoreWebView2InitializationCompleted += CoreWebView_Loaded;
	}

	private void SetSize() {
		var bounds = Screen.AllScreens.Select(screen => screen.Bounds);
		var left = 0;
		var top = 0;
		var right = 0;
		var bottom = 0;
		foreach (var bound in bounds) {
			if (bound.Left < left) left = bound.Left;
			if (bound.Top < top) top = bound.Top;
			if (bound.Right > right) right = bound.Right;
			if (bound.Bottom > bottom) bottom = bound.Bottom;
		}

		var handle = new WindowInteropHelper(this).Handle;
		_ = NativeMethods.SetWindowPos(handle, 0, left, top, right - left, bottom - top, 0);
	}

	private void SetWindowStyle() {
		const int gwlExStyle = -20; // GWL_EXSTYLE
		const uint wsExToolWindow = 0x00000080; // WS_EX_TOOLWINDOW

		var handle = new WindowInteropHelper(this).Handle;
		_ = NativeMethods.SetWindowLongPtr(handle, gwlExStyle, NativeMethods.GetWindowLongPtr(handle, gwlExStyle) | (nint)wsExToolWindow);
	}

	private void WebView_Loaded(object? sender, CoreWebView2NavigationCompletedEventArgs e) {
		// If we are running in a development/debugger mode, open dev tools to help out
		if (Debugger.IsAttached) blazorOverlayWebView.WebView.CoreWebView2.OpenDevToolsWindow();
	}

	private void CoreWebView_Loaded(object? sender, CoreWebView2InitializationCompletedEventArgs e) {
		blazorOverlayWebView.WebView.CoreWebView2.SetVirtualHostNameToFolderMapping("local.data", "Data", CoreWebView2HostResourceAccessKind.Allow);
		blazorOverlayWebView.WebView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = false;
		blazorOverlayWebView.WebView.CoreWebView2.Settings.AreBrowserAcceleratorKeysEnabled = false;
	}

	private static partial class NativeMethods {
		[LibraryImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
		public static partial nint GetWindowLongPtr(nint hWnd, int nIndex);

		[LibraryImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
		public static partial nint SetWindowLongPtr(nint hWnd, int nIndex, nint dwNewLong);

		[LibraryImport("user32.dll", EntryPoint = "SetWindowPos", SetLastError = true)]
		[return: MarshalAs(UnmanagedType.Bool)]
		public static partial bool SetWindowPos(nint hWnd, nint hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
	}
}
