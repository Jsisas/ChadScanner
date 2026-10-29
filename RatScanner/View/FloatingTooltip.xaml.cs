using RatEye;
using RatScanner.TarkovDev.Json;
using RatScanner.ViewModel;
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.Linq;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Forms;
using Application = System.Windows.Application;
using Brushes = System.Windows.Media.Brushes;
using Label = System.Windows.Controls.Label;
using MouseEventArgs = System.Windows.Input.MouseEventArgs;
using RcFloatingTooltip = RatScanner.RatConfig.FloatingTooltip;
using Timer = System.Windows.Forms.Timer;

namespace RatScanner.View;

public partial class FloatingTooltip : Window {
	private static FloatingTooltip _instance = null!;
	public static FloatingTooltip Instance => _instance ??= new FloatingTooltip();

	private const int MouseMovementUntilHide = 10;

	private Vector2? mouseStartPosition;
	private string LastItemName;
	private int ItemScansCount;

	public event PropertyChangedEventHandler PropertyChanged;

	Timer fadeTimer;

	private FloatingTooltip() {
		InitializeComponent();
		Hide();
		MenuVM dataContext = new(RatScannerMain.Instance);
		DataContext = dataContext;
		LastItemName = dataContext.LastItem.Name;
		ItemScansCount = dataContext.ItemScans.Count;
		dataContext.PropertyChanged += OnPropertyChanged;
		UpdateElements();
	}

	protected override void OnMouseLeave(MouseEventArgs e) {
		Instance.HideTooltip();
	}

	protected override void OnMouseMove(MouseEventArgs e) {
		Vector2 mousePosition = UserActivityHelper.GetMousePosition();
		if (mouseStartPosition == null) {
			mouseStartPosition = mousePosition;
		} else if (Int32.Abs((mousePosition.X - mouseStartPosition.X)) > MouseMovementUntilHide ||
				   Int32.Abs((mousePosition.Y - mouseStartPosition.Y)) > MouseMovementUntilHide) {
			Application.Current.Dispatcher.Invoke(() => {
				Instance.HideTooltip();
			});
		}
	}

	protected virtual void OnPropertyChanged(object? sender, PropertyChangedEventArgs e) {
		if (RcFloatingTooltip.Enable && sender is MenuVM vm) {
			if ((!vm.LastItem.Name.Equals(LastItemName) || vm.ItemScans.Count > ItemScansCount)) {
				LastItemName = vm.LastItem.Name;
				ItemScansCount = vm.ItemScans.Count;
				Application.Current.Dispatcher.Invoke(() => Instance.ShowTooltip());
			}
		}
	}

	private void HideTooltip() {
		mouseStartPosition = null;
		Hide();
	}

	private void ShowTooltip() {
		MenuVM context = ((MenuVM)Instance.DataContext);

		TaskItemList.Children.Clear();
		if (context.TaskItemRemaining.Count > 0 && RcFloatingTooltip.ShowTasksInfo) {
			TaskItemList.Children.Add(new Separator());

			Label hideoutTitle = new();
			hideoutTitle.Foreground = Brushes.Orange;
			hideoutTitle.Content = "Tasks: " + context.TaskRemaining + "x";
			TaskItemList.Children.Add(hideoutTitle);

			IOrderedEnumerable<(TaskItemRemaining remaining, string trader)> sortedTaskItemRemaining = context.TaskItemRemaining
				.Select(x => (remaining: x, trader: TarkovDevAPI.GetTraderName(x.Task.Trader)))
				.OrderBy(x => x.trader).ThenBy(x => x.remaining.Task.MinPlayerLevel);
			foreach ((TaskItemRemaining taskItemRemaining, string trader) in sortedTaskItemRemaining) {
				Label label = new();
				label.Content = "[" + taskItemRemaining.Task.MinPlayerLevel + "] " + trader + " => " + taskItemRemaining.Task.Name + " " + taskItemRemaining.ItemCount + "x";
				TaskItemList.Children.Add(label);
			}
		}

		HideoutItems.Children.Clear();
		if (context.HideoutItemRemaining.Count > 0 && RcFloatingTooltip.ShowHideoutInfo) {
			HideoutItems.Children.Add(new Separator());

			Label hideoutTitle = new();
			hideoutTitle.Foreground = Brushes.Orange;
			hideoutTitle.Content = "Hideout: " + context.HideoutRemaining + "x";
			HideoutItems.Children.Add(hideoutTitle);

			System.Collections.ObjectModel.ObservableCollection<HideoutItemRemaining> sortedHideoutItemRemaining = context.HideoutItemRemaining;
			foreach (HideoutItemRemaining hideoutItemRemaining in sortedHideoutItemRemaining) {
				Label label = new();
				label.Content = "[" + hideoutItemRemaining.Level + "] " + hideoutItemRemaining.Name + " " + hideoutItemRemaining.ItemCount + "x";
				HideoutItems.Children.Add(label);
			}
		}


		BarterItemList.Children.Clear();
		CraftItemList.Children.Clear();
		List<Barter> barters = RcFloatingTooltip.ShowBarterInfo ? context.LastItem.GetBartersUsing().ToList() : new();
		List<Craft> crafts = RcFloatingTooltip.ShowCraftsInfo ? context.LastItem.GetCraftsUsing().ToList() : new();
		Dictionary<string, string> itemNames = barters.Count + crafts.Count > 0
			? TarkovDevAPI.GetItems().ToDictionary(i => i.Id, i => i.Name)
			: new();

		if (barters.Count > 0) {
			BarterItemList.Children.Add(new Separator());

			Label barterTitle = new();
			barterTitle.Foreground = Brushes.Orange;
			barterTitle.Content = "Barters";
			BarterItemList.Children.Add(barterTitle);

			var sortedBarters = barters
				.Select(b => (barter: b, trader: TarkovDevAPI.GetTraderName(b.Trader)))
				.OrderBy(x => x.trader).ThenBy(x => x.barter.MinTraderLevel);
			foreach ((Barter barter, string trader) in sortedBarters) {
				Label label = new();
				label.Content = FormatRecipe(barter.MinTraderLevel, trader, barter.RequiredItems, barter.OfferedItem, itemNames);
				BarterItemList.Children.Add(label);
			}
		}

		if (crafts.Count > 0) {
			CraftItemList.Children.Add(new Separator());

			Label craftTitle = new();
			craftTitle.Foreground = Brushes.Orange;
			craftTitle.Content = "Crafts";
			CraftItemList.Children.Add(craftTitle);

			var sortedCrafts = crafts
				.Select(c => (craft: c, station: TarkovDevAPI.GetHideoutStationName(c.Station)))
				.OrderBy(x => x.station).ThenBy(x => x.craft.Level);
			foreach ((Craft craft, string station) in sortedCrafts) {
				Label label = new();
				label.Content = FormatRecipe(craft.Level, station, craft.RequiredItems, craft.ProductItem, itemNames);
				CraftItemList.Children.Add(label);
			}
		}

		Vector2 mousePosition = UserActivityHelper.GetMousePosition();
		Instance.Top = Math.Min(RatConfig.ScreenHeight, mousePosition.Y - MouseMovementUntilHide + Instance.Height) - Instance.Height;
		Instance.Left = Math.Min(RatConfig.ScreenWidth, mousePosition.X - MouseMovementUntilHide + Instance.Width) - Instance.Width;

		Instance.Show();
	}

	private static string FormatRecipe(int level, string source, List<RecipeItem> required, RecipeItem? product, Dictionary<string, string> itemNames) {
		string Name(string id) => itemNames.TryGetValue(id, out string? name) ? name : id;

		StringBuilder sb = new();
		sb.Append('[').Append(level).Append("] ").Append(source).Append(" => ");
		sb.AppendJoin(" + ", required.Select(r => $"{Name(r.Item)} {r.Count}x"));
		if (product != null) sb.Append(" == ").Append(Name(product.Item)).Append(' ').Append(product.Count).Append('x');
		return sb.ToString();
	}

	public void UpdateElements() {
		const Visibility v = Visibility.Visible;
		const Visibility c = Visibility.Collapsed;

		NameDisplay.Visibility = RcFloatingTooltip.ShowName ? v : c;
		AvgDayPriceDisplay.Visibility = RcFloatingTooltip.ShowAvgDayPrice ? v : c;
		PricePerSlotDisplay.Visibility = RcFloatingTooltip.ShowPricePerSlot ? v : c;
		TraderPriceDisplay.Visibility = RcFloatingTooltip.ShowTraderPrice ? v : c;
		UpdatedDisplay.Visibility = RcFloatingTooltip.ShowUpdated ? v : c;
		TaskItemList.Visibility = RcFloatingTooltip.ShowTasksInfo ? v : c;
		HideoutItems.Visibility = RcFloatingTooltip.ShowHideoutInfo ? v : c;
		BarterItemList.Visibility = RcFloatingTooltip.ShowBarterInfo ? v : c;
		CraftItemList.Visibility = RcFloatingTooltip.ShowCraftsInfo ? v : c;
	}
}
