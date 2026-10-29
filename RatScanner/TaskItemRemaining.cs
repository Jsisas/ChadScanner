using RatScanner.TarkovDev.Json;

namespace RatScanner;

public class TaskItemRemaining {
	public TaskItemRemaining(int itemCount, TarkovTask task) {
		ItemCount = itemCount;
		Task = task;
	}

	public int ItemCount { get; set; }
	public TarkovTask Task { get; set; }

}
