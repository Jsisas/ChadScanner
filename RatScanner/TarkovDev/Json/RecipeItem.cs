using Newtonsoft.Json;
using System.Collections.Generic;

namespace RatScanner.TarkovDev.Json;

/// <summary>
/// Item entry of a barter or craft. Count is fractional for some barters (e.g. currency amounts).
/// </summary>
public class RecipeItem {
	[JsonProperty("item")]
	public string ItemId { get; set; } = string.Empty;
	public Item? Item => RatScanner.TaskExtensions.GetItemLookup().GetValueOrDefault(ItemId);

	[JsonProperty("count")]
	public double Count { get; set; }

	[JsonProperty("attributes")]
	public Dictionary<string, object>? Attributes { get; set; }
}
