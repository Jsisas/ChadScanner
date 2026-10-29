using RatScanner.TarkovDev.Json;

namespace RatScanner;

/// <summary>Remaining amount of an item a single task still needs.</summary>
public record TaskItemRemaining(int ItemCount, TarkovTask Task);

/// <summary>Remaining amount of an item a single unbuilt hideout station level still needs.</summary>
public record HideoutItemRemaining(int ItemCount, string Name, int Level);
