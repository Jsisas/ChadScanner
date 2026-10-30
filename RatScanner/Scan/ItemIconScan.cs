using RatEye;
using RatStash;
using System;
using System.Linq;
using Icon = RatEye.Processing.Icon;

namespace RatScanner.Scan;

public class ItemIconScan : ItemScan {
	public bool Rotated;

	/// <summary>Size of the scanned item on screen, in pixels.</summary>
	public Vector2 ItemSize;

	private readonly Vector2 _toolTipPosition;

	public ItemIconScan(Icon icon, Vector2 toolTipPosition, int duration) : this(icon, LookupItem(icon.Item.Id), icon.DetectionConfidence, toolTipPosition, duration) { }

	public ItemIconScan(Icon icon, RatScanner.TarkovDev.Json.Item item, float confidence, Vector2 toolTipPosition, int duration)
		: this(item, confidence, icon.Rotated, icon.ItemSize, toolTipPosition, duration) {
		// RatEye's icon only matches when ChadVision agreed; otherwise the overlay falls back to item.BaseImageLink.
		IconPath = icon.Item?.Id == item.Id ? icon.IconPath : string.Empty;
	}

	/// <summary>
	/// Scan identified without a RatEye icon, e.g. by ChadVision alone from the stash grid.
	/// </summary>
	public ItemIconScan(RatScanner.TarkovDev.Json.Item item, float confidence, bool rotated, Vector2 itemSize, Vector2 toolTipPosition, int duration) {
		Item = item;
		Confidence = confidence;
		Rotated = rotated;
		ItemSize = itemSize;
		_toolTipPosition = toolTipPosition;
		DissapearAt = DateTimeOffset.Now.ToUnixTimeMilliseconds() + duration;
	}

	private static RatScanner.TarkovDev.Json.Item LookupItem(string id) {
		return TarkovDevAPI.GetItems().FirstOrDefault(item => item.Id == id) ?? throw new Exception($"Unknown item: {id}");
	}

	public override Vector2 GetToolTipPosition() {
		return _toolTipPosition;
	}
}
