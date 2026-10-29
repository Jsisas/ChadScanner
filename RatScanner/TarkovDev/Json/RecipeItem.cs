using Newtonsoft.Json;
using System.Collections.Generic;

namespace RatScanner.TarkovDev.Json;

/// <summary>
/// Item entry of a barter or craft. Count is fractional for some barters (e.g. currency amounts).
/// </summary>
public class RecipeItem {
	[JsonProperty("item")]
	public string Item { get; set; } = string.Empty;

	[JsonProperty("count")]
	public double Count { get; set; }

	[JsonProperty("attributes")]
	public Dictionary<string, object>? Attributes { get; set; }
}
