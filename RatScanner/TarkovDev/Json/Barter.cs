using Newtonsoft.Json;
using System.Collections.Generic;
using System.Linq;

namespace RatScanner.TarkovDev.Json;

public class Barter {
	[JsonProperty("id")]
	public string Id { get; set; } = string.Empty;

	[JsonProperty("trader")]
	public string TraderId { get; set; } = string.Empty;
	public Trader? Trader => TarkovDevAPI.GetTraders().FirstOrDefault(t => t.Id == TraderId);

	[JsonProperty("minTraderLevel")]
	public int MinTraderLevel { get; set; }

	[JsonProperty("taskUnlock")]
	public string? TaskUnlock { get; set; }

	[JsonProperty("requiredItems")]
	public List<RecipeItem> RequiredItems { get; set; } = [];

	[JsonProperty("offeredItem")]
	public RecipeItem? OfferedItem { get; set; }
}
