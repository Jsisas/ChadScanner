using Newtonsoft.Json;
using System.Collections.Generic;

namespace RatScanner.TarkovDev.Json;

public class Craft {
	[JsonProperty("id")]
	public string Id { get; set; } = string.Empty;

	[JsonProperty("station")]
	public string Station { get; set; } = string.Empty;

	[JsonProperty("level")]
	public int Level { get; set; }

	[JsonProperty("duration")]
	public int Duration { get; set; }

	[JsonProperty("requiredItems")]
	public List<RecipeItem> RequiredItems { get; set; } = new();

	[JsonProperty("productItem")]
	public RecipeItem? ProductItem { get; set; }
}
