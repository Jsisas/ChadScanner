using Newtonsoft.Json;
using System.Collections.Generic;
using System.Linq;

namespace RatScanner.TarkovDev.Json;

public class Craft {
	[JsonProperty("id")]
	public string Id { get; set; } = string.Empty;

	[JsonProperty("station")]
	public string StationId { get; set; } = string.Empty;
	public HideoutStation? Station => TarkovDevAPI.GetHideoutStations().FirstOrDefault(s => s.Id == StationId);

	[JsonProperty("level")]
	public int Level { get; set; }

	[JsonProperty("duration")]
	public int Duration { get; set; }

	[JsonProperty("requiredItems")]
	public List<RecipeItem> RequiredItems { get; set; } = [];

	[JsonProperty("productItem")]
	public RecipeItem? ProductItem { get; set; }
}
