using RatEye;
using RatScanner.Scan;
using RatScanner.TarkovDev.Json;
using RatScanner.ViewModel;
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using Brushes = System.Windows.Media.Brushes;
using MouseEventArgs = System.Windows.Input.MouseEventArgs;
using RcFloatingTooltip = RatScanner.RatConfig.FloatingTooltip;

namespace RatScanner.View;

/// <summary>
/// Small window shown at the cursor after every scan, listing where the item is needed
/// (tasks, hideout) and what it can be used for (barters, crafts).
/// </summary>
public partial class FloatingTooltip : Window {
	public static FloatingTooltip Instance { get => field ??= new FloatingTooltip(); }

	private const int MouseMovementUntilHide = 10;

	private Vector2? _mouseStartPosition;
	private readonly MenuVM _vm;

	private FloatingTooltip() {
		InitializeComponent();
		Hide();
		_vm = new MenuVM(RatScannerMain.Instance);
		DataContext = _vm;
		RatScannerMain.Instance.ItemScans.Enqueued += OnItemScanEnqueued;
		UpdateElements();
	}

	protected override void OnMouseLeave(MouseEventArgs e) => HideTooltip();

	protected override void OnMouseMove(MouseEventArgs e) {
		var mousePosition = UserActivityHelper.GetMousePosition();
		if (_mouseStartPosition == null) {
			_mouseStartPosition = mousePosition;
		} else if (Math.Abs(mousePosition.X - _mouseStartPosition.X) > MouseMovementUntilHide ||
				   Math.Abs(mousePosition.Y - _mouseStartPosition.Y) > MouseMovementUntilHide) {
			HideTooltip();
		}
	}

	// Scans are enqueued from the hotkey/scan threads; windows can only be touched on the UI thread.
	private void OnItemScanEnqueued(object? sender, ItemScan scan) {
		if (!RcFloatingTooltip.Enable) return;
		_ = Dispatcher.BeginInvoke(ShowTooltip);
	}

	private void HideTooltip() {
		_mouseStartPosition = null;
		if (!IsVisible) return;
		// A transparent window re-presents its last rendered frame when shown again, so render
		// it empty before hiding; otherwise the next scan flashes the previous item first.
		DataContext = null;
		TaskItemList.Children.Clear();
		HideoutItems.Children.Clear();
		BarterItemList.Children.Clear();
		CraftItemList.Children.Clear();
		Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.Render);
		Hide();
	}

	private void ShowTooltip() {
		// Enqueued fires before RatScannerMain raises PropertyChanged, so the bindings still
		// hold the previous item. Re-setting the DataContext re-evaluates them now, before
		// the window is measured and shown; otherwise the old item flashed first.
		DataContext = null;
		DataContext = _vm;
		var item = _vm.LastItem;

		// Set here rather than by a style trigger: the local value from UpdateElements would override a trigger
		TraderPriceDisplay.Visibility = RcFloatingTooltip.ShowTraderPrice && item.GetBestSellToTraderOffer() != null
			? Visibility.Visible
			: Visibility.Collapsed;

		TaskItemList.Children.Clear();
		if (RcFloatingTooltip.ShowTasksInfo) {
			var tasks = item.GetTaskItemRemaining()
				.Select(t => (remaining: t, trader: t.Task.Trader?.Name ?? ""))
				.OrderBy(t => t.trader).ThenBy(t => t.remaining.Task.MinPlayerLevel)
				.ToList();
			if (tasks.Count > 0) {
				AddSection(TaskItemList, $"{Translate("TooltipTasks")}: {tasks.Sum(t => t.remaining.ItemCount)}x");
				foreach (var (remaining, trader) in tasks) {
					AddLine(TaskItemList, $"[{remaining.Task.MinPlayerLevel}] {trader} => {remaining.Task.Name} {remaining.ItemCount}x");
				}
			}
		}

		HideoutItems.Children.Clear();
		if (RcFloatingTooltip.ShowHideoutInfo) {
			var hideout = item.GetHideoutItemRemaining();
			if (hideout.Count > 0) {
				AddSection(HideoutItems, $"{Translate("TooltipHideout")}: {hideout.Sum(h => h.ItemCount)}x");
				foreach (var h in hideout) AddLine(HideoutItems, $"[{h.Level}] {h.Name} {h.ItemCount}x");
			}
		}

		BarterItemList.Children.Clear();
		if (RcFloatingTooltip.ShowBarterInfo) {
			var barters = item.GetBartersUsing()
				.Select(b => (barter: b, trader: b.Trader?.Name ?? ""))
				.OrderBy(b => b.trader).ThenBy(b => b.barter.MinTraderLevel)
				.ToList();
			if (barters.Count > 0) {
				AddSection(BarterItemList, Translate("TooltipBarters"));
				foreach (var (barter, trader) in barters) {
					AddLine(BarterItemList, FormatRecipe(barter.MinTraderLevel, trader, barter.RequiredItems, barter.OfferedItem));
				}
			}
		}

		CraftItemList.Children.Clear();
		if (RcFloatingTooltip.ShowCraftsInfo) {
			var crafts = item.GetCraftsUsing()
				.Select(c => (craft: c, station: c.Station?.Name ?? ""))
				.OrderBy(c => c.station).ThenBy(c => c.craft.Level)
				.ToList();
			if (crafts.Count > 0) {
				AddSection(CraftItemList, Translate("TooltipCrafts"));
				foreach (var (craft, station) in crafts) {
					AddLine(CraftItemList, FormatRecipe(craft.Level, station, craft.RequiredItems, craft.ProductItem));
				}
			}
		}

		// Size the window from the measured content before Show(): with SizeToContent the window
		// opened at its last (emptied) size and grew a layout pass later, visibly jumping.
		var content = (FrameworkElement)Content;
		content.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
		var size = new Size(Math.Ceiling(content.DesiredSize.Width), Math.Ceiling(content.DesiredSize.Height));
		Width = size.Width;
		Height = size.Height;

		// Mouse and screen size are physical pixels; Left/Top are DIPs.
		var dpi = RatConfig.ScreenScale > 0 ? RatConfig.ScreenScale : 1f;
		var mousePosition = UserActivityHelper.GetMousePosition();
		var screenWidth = RatConfig.ScreenWidth / dpi;
		var screenHeight = RatConfig.ScreenHeight / dpi;
		Top = Math.Max(0, Math.Min(screenHeight, mousePosition.Y / dpi - MouseMovementUntilHide + size.Height) - size.Height);
		Left = Math.Max(0, Math.Min(screenWidth, mousePosition.X / dpi - MouseMovementUntilHide + size.Width) - size.Width);

		Show();
	}

	private static void AddSection(Panel panel, string title) {
		_ = panel.Children.Add(new Separator());
		_ = panel.Children.Add(new Label { Foreground = Brushes.Orange, Content = title });
	}

	private static void AddLine(Panel panel, string text) => _ = panel.Children.Add(new Label { Content = text });

	private static string FormatRecipe(int level, string source, List<RecipeItem> required, RecipeItem? product) {
		static string Entry(RecipeItem r) => $"{r.Item?.Name ?? r.ItemId} {r.Count.ToString(CultureInfo.CurrentCulture)}x";

		StringBuilder sb = new();
		_ = sb.Append('[').Append(level).Append("] ").Append(source).Append(" => ");
		_ = sb.AppendJoin(" + ", required.Select(Entry));
		if (product != null) _ = sb.Append(" == ").Append(Entry(product));
		return sb.ToString();
	}

	private static string Translate(string key) => LocalizationService.Translate(key);

	/// <summary>
	/// Applies the visibility and label settings. Called on creation and after settings are saved.
	/// </summary>
	public void UpdateElements() {
		const Visibility v = Visibility.Visible;
		const Visibility c = Visibility.Collapsed;

		AvgDayPriceLabel.Content = Translate("AvgDailyPrice");
		PricePerSlotLabel.Content = Translate("PricePerSlot");
		UpdatedLabel.Content = Translate("TooltipUpdated");

		NameDisplay.Visibility = RcFloatingTooltip.ShowName ? v : c;
		AvgDayPriceDisplay.Visibility = RcFloatingTooltip.ShowAvgDayPrice ? v : c;
		PricePerSlotDisplay.Visibility = RcFloatingTooltip.ShowPricePerSlot ? v : c;
		UpdatedDisplay.Visibility = RcFloatingTooltip.ShowUpdated ? v : c;
		TaskItemList.Visibility = RcFloatingTooltip.ShowTasksInfo ? v : c;
		HideoutItems.Visibility = RcFloatingTooltip.ShowHideoutInfo ? v : c;
		BarterItemList.Visibility = RcFloatingTooltip.ShowBarterInfo ? v : c;
		CraftItemList.Visibility = RcFloatingTooltip.ShowCraftsInfo ? v : c;

		if (!RcFloatingTooltip.Enable) HideTooltip();
	}
}
