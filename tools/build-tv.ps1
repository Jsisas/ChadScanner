# Builds the tarkov-vision native library and stages its data for RatScanner.
# Run from the repository root: powershell -File tools\build-tv.ps1 [-TvRepo ..\tarkov-vision]
param(
	[string]$TvRepo = "..\tarkov-vision"
)

$ErrorActionPreference = "Stop"

function Invoke-Checked([scriptblock]$Command) {
	& $Command
	if ($LASTEXITCODE -ne 0) { throw "Command failed with exit code ${LASTEXITCODE}: $Command" }
}

$RepoRoot = (Get-Location).Path
$TvRepo = (Resolve-Path $TvRepo).Path
$NativeDir = Join-Path $RepoRoot "RatScanner\Native"
$DataDir = Join-Path $RepoRoot "RatScanner\Data\tv"

Push-Location $TvRepo
try {
	Write-Host "Building tv library..."
	Invoke-Checked { cargo build --release --lib }

	if (-not (Test-Path "data\refs\catalog.json")) {
		Write-Host "Fetching reference icons..."
		Invoke-Checked { cargo run --release -- fetch-refs --out data\refs }
	}
	if (-not (Test-Path "data\models\dinov2-small.onnx")) {
		Write-Host "Fetching models..."
		Invoke-Checked { cargo run --release -- fetch-models }
	}
	if (-not (Test-Path "data\index")) {
		Write-Host "Building index..."
		Invoke-Checked { cargo run --release -- bench --set data/real --methods fusion }
	}

	New-Item -ItemType Directory -Force $NativeDir | Out-Null
	Copy-Item "target\release\tv.dll" $NativeDir -Force
	Get-ChildItem "target\release" -Filter "onnxruntime*.dll" -ErrorAction SilentlyContinue |
		Copy-Item -Destination $NativeDir -Force

	Write-Host "Copying data to $DataDir..."
	New-Item -ItemType Directory -Force (Join-Path $DataDir "refs") | Out-Null
	Copy-Item "data\refs\catalog.json" (Join-Path $DataDir "refs") -Force
	Copy-Item "data\refs\base" (Join-Path $DataDir "refs") -Recurse -Force
	Copy-Item "data\models" $DataDir -Recurse -Force
	Copy-Item "data\index" $DataDir -Recurse -Force
}
finally {
	Pop-Location
}

Write-Host "tarkov-vision ready: $NativeDir, $DataDir"
