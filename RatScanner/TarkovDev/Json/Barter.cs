using Newtonsoft.Json;
using System.Collections.Generic;

namespace RatScanner.TarkovDev.Json;

public class Barter {
	[JsonProperty("id")]
	public string Id { get; set; } = string.Empty;

	[JsonProperty("trader")]
	public string Trader { get; set; } = string.Empty;

	[JsonProperty("minTraderLevel")]
	public int MinTraderLevel { get; set; }

	[JsonProperty("taskUnlock")]
	public string? TaskUnlock { get; set; }

	[JsonProperty("requiredItems")]
	public List<RecipeItem> RequiredItems { get; set; } = new();

	[JsonProperty("offeredItem")]
	public RecipeItem? OfferedItem { get; set; }
}
