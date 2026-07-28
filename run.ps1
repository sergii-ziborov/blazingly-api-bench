<#
.SYNOPSIS
    Runs the API comparison: builds, verifies equivalence, then measures.

.DESCRIPTION
    Equivalence is checked before anything is measured. Comparing the
    throughput of servers that return different things is not a benchmark, so a
    disagreement aborts the run instead of being reported.

    Scenarios are interleaved by default: every implementation runs sample 1,
    then every implementation runs sample 2, and so on. On a machine carrying
    unrelated load this matters more than the sample count, because a drift in
    background CPU then hits all implementations rather than whichever one
    happened to run during it.
#>
param(
    [ValidateRange(1, 4096)]
    [int]$Connections = 64,
    [ValidateRange(1, 600)]
    [int]$DurationSeconds = 8,
    [ValidateRange(0, 60)]
    [int]$WarmupSeconds = 2,
    [ValidateRange(1, 20)]
    [int]$Rounds = 3,
    [ValidateRange(1, 64)]
    [int]$Workers = 4,
    [ValidateSet("all", "list", "detail", "filter", "search", "bulk")]
    [string]$Scenario = "all",
    [ValidateSet("all", "blazingly", "axum", "actix", "fastapi")]
    [string]$Framework = "all",
    [string]$PythonExecutable = "",
    [switch]$SkipVerify,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repoRoot = $PSScriptRoot
$rustRoot = Join-Path $repoRoot "rust"
$loadgen = Join-Path $rustRoot "target\release\apibench-loadgen.exe"
$resultsPath = Join-Path $repoRoot "results"
New-Item -ItemType Directory -Force -Path $resultsPath | Out-Null

if ([string]::IsNullOrWhiteSpace($PythonExecutable)) {
    $candidate = Join-Path $repoRoot "python\fastapi-api\.venv\Scripts\python.exe"
    if (Test-Path $candidate) {
        $PythonExecutable = $candidate
    } else {
        $PythonExecutable = "C:\Users\SergiiZiborov\Documents\GitHub\MyProjects\blazingly-benchmarks\.venv\Scripts\python.exe"
    }
}

$servers = @(
    @{ Name = "blazingly"; Port = 3201; Kind = "rust"; Package = "blazingly-api"; Exe = "blazingly-api.exe" }
    @{ Name = "axum";      Port = 3202; Kind = "rust"; Package = "axum-api";      Exe = "axum-api.exe" }
    @{ Name = "actix";     Port = 3203; Kind = "rust"; Package = "actix-api";     Exe = "actix-api.exe" }
    @{ Name = "fastapi";   Port = 3205; Kind = "python" }
)
if ($Framework -ne "all") {
    $servers = $servers | Where-Object { $_.Name -eq $Framework }
}

# method, path, body file, headers, expected status, mutating
$scenarios = @{
    list   = @{ Method = "GET";  Path = "/articles?page=3&limit=20"; Body = $null; Headers = @(); Status = 200; Mutating = $false }
    detail = @{ Method = "GET";  Path = "/articles/article-0042";    Body = $null; Headers = @(); Status = 200; Mutating = $false }
    filter = @{ Method = "GET";  Path = "/articles?category=startups&lang=uk&limit=20"; Body = $null; Headers = @(); Status = 200; Mutating = $false }
    search = @{ Method = "GET";  Path = "/search?q=ai";              Body = $null; Headers = @(); Status = 200; Mutating = $false }
    bulk   = @{ Method = "POST"; Path = "/ingest/articles/bulk";     Body = "payloads/bulk50.json"; Headers = @("x-api-key: scraper-key", "content-type: application/json"); Status = 200; Mutating = $true }
}
$selected = if ($Scenario -eq "all") { @("list", "detail", "filter", "search", "bulk") } else { @($Scenario) }

function Get-ProcessTreeIds {
    param([int]$RootProcessId)
    $ids = [System.Collections.Generic.List[int]]::new()
    $ids.Add($RootProcessId)
    $pending = [System.Collections.Generic.Queue[int]]::new()
    $pending.Enqueue($RootProcessId)
    while ($pending.Count -gt 0) {
        $current = $pending.Dequeue()
        foreach ($child in (Get-CimInstance Win32_Process -Filter "ParentProcessId = $current" -ErrorAction SilentlyContinue)) {
            $childId = [int]$child.ProcessId
            if (-not $ids.Contains($childId)) { $ids.Add($childId); $pending.Enqueue($childId) }
        }
    }
    return $ids
}

function Get-ProcessTreeUsage {
    param([int]$RootProcessId)
    $cpu = 0.0; $peak = 0
    foreach ($id in (Get-ProcessTreeIds -RootProcessId $RootProcessId)) {
        try {
            $process = Get-Process -Id $id -ErrorAction Stop
            $cpu += $process.TotalProcessorTime.TotalSeconds
            $peak += $process.PeakWorkingSet64
        } catch { continue }
    }
    return [PSCustomObject]@{ CpuSeconds = $cpu; PeakBytes = $peak }
}

function Start-Server {
    param($Server)
    $env:BLAZINGLY_BENCH_WORKERS = $Workers.ToString()
    $env:BLAZINGLY_APIBENCH_SEED = (Join-Path $repoRoot "data\seed.json")
    # The contract's 100 req/s ingestion budget would make the bulk scenario
    # measure the rate limiter instead of validation, so the harness raises it.
    # All four implementations read this one variable; raising it for some and
    # not others would silently compare different things.
    $env:APIBENCH_INGEST_RPS = "100000000"
    if ($Server.Kind -eq "rust") {
        $exe = Join-Path $rustRoot "target\release\$($Server.Exe)"
        return Start-Process -FilePath $exe -PassThru -WindowStyle Hidden -WorkingDirectory $rustRoot
    }
    $appDir = Join-Path $repoRoot "python\fastapi-api"
    return Start-Process -FilePath $PythonExecutable -PassThru -WindowStyle Hidden `
        -WorkingDirectory $appDir `
        -ArgumentList @("-m", "uvicorn", "main:app", "--host", "127.0.0.1",
            "--port", "$($Server.Port)", "--workers", "$Workers", "--no-access-log")
}

function Wait-Ready {
    param($Server, [int]$AttemptLimit = 900)
    for ($i = 0; $i -lt $AttemptLimit; $i++) {
        try {
            $null = Invoke-WebRequest -Uri "http://127.0.0.1:$($Server.Port)/health" -UseBasicParsing -TimeoutSec 2
            return $true
        } catch { Start-Sleep -Milliseconds 40 }
    }
    return $false
}

function Stop-Server {
    param($Process)
    if ($Process -and -not $Process.HasExited) {
        & taskkill.exe /PID $Process.Id /T /F | Out-Null
        $Process.WaitForExit(10000) | Out-Null
    }
}

if (-not $SkipBuild) {
    Write-Host "building rust implementations..."
    cargo build --release -j 3 --manifest-path (Join-Path $rustRoot "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "rust build failed" }
}

if (-not $SkipVerify) {
    Write-Host "starting every implementation for the equivalence check..."
    $running = @{}
    try {
        foreach ($server in $servers) {
            $process = Start-Server -Server $server
            $running[$server.Name] = $process
            if (-not (Wait-Ready -Server $server)) { throw "$($server.Name) never became ready" }
            Write-Host "  $($server.Name) ready on $($server.Port)"
        }
        $targets = $servers | ForEach-Object { "$($_.Name)=$($_.Port)" }
        & $PythonExecutable (Join-Path $repoRoot "tools\verify_equivalence.py") @targets
        if ($LASTEXITCODE -ne 0) {
            throw "implementations are not equivalent; refusing to report a benchmark"
        }
    } finally {
        foreach ($process in $running.Values) { Stop-Server -Process $process }
    }
}

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$samples = @{}
foreach ($server in $servers) { foreach ($name in $selected) { $samples["$($server.Name)/$name"] = @() } }

foreach ($round in 1..$Rounds) {
    foreach ($name in $selected) {
        $case = $scenarios[$name]
        foreach ($server in $servers) {
            $process = $null
            try {
                $process = Start-Server -Server $server
                if (-not (Wait-Ready -Server $server)) {
                    Write-Warning "$($server.Name) never became ready; skipping"
                    continue
                }
                $hostCpu = [Math]::Round((((Get-Counter "\Processor(_Total)\% Processor Time" `
                    -SampleInterval 1 -MaxSamples 2).CounterSamples |
                    Measure-Object -Property CookedValue -Average).Average), 1)

                $arguments = @(
                    "--address", "127.0.0.1:$($server.Port)"
                    "--method", $case.Method
                    "--path", $case.Path
                    "--connections", $Connections
                    "--duration", $DurationSeconds
                    "--warmup", $WarmupSeconds
                    "--expect-status", $case.Status
                )
                foreach ($header in $case.Headers) { $arguments += @("--header", $header) }
                if ($case.Body) {
                    $arguments += @("--body-file", (Join-Path $repoRoot $case.Body))
                }

                $before = Get-ProcessTreeUsage -RootProcessId $process.Id
                $watch = [System.Diagnostics.Stopwatch]::StartNew()
                $output = & $loadgen @arguments
                $exit = $LASTEXITCODE
                $watch.Stop()
                $after = Get-ProcessTreeUsage -RootProcessId $process.Id

                $rps = ($output | Select-String '^Requests/sec:').Line
                $p50 = ($output | Select-String '^Latency p50').Line
                $p99 = ($output | Select-String '^Latency p99:').Line
                if ($exit -ne 0 -or -not $rps) {
                    Write-Warning "$($server.Name)/$name round $round failed"
                    $output | Select-Object -Last 4 | ForEach-Object { Write-Warning "  $_" }
                    continue
                }
                $value = [double](($rps -split "`t")[-1])
                $cpuPercent = 100 * ($after.CpuSeconds - $before.CpuSeconds) / $watch.Elapsed.TotalSeconds
                $samples["$($server.Name)/$name"] += [PSCustomObject]@{
                    Rps = $value
                    P50 = ($p50 -split "`t")[-1]
                    P99 = ($p99 -split "`t")[-1]
                    CpuPercent = [Math]::Round($cpuPercent, 0)
                    PeakMiB = [Math]::Round($after.PeakBytes / 1MB, 1)
                    HostCpu = $hostCpu
                }
                "{0,-10} {1,-7} round {2}  {3,10:N0} rps  p50 {4,-10} p99 {5,-10} cpu {6,4:N0}%  rss {7,6:N1} MiB  host {8}%" -f `
                    $server.Name, $name, $round, $value, ($p50 -split "`t")[-1], ($p99 -split "`t")[-1],
                    $cpuPercent, ($after.PeakBytes / 1MB), $hostCpu
            } finally {
                Stop-Server -Process $process
                Start-Sleep -Milliseconds 400
            }
        }
    }
}

$report = Join-Path $resultsPath "$timestamp-summary.txt"
"API comparison: $Connections connections, ${DurationSeconds}s per sample, ${WarmupSeconds}s warmup, $Rounds rounds, $Workers workers" |
    Tee-Object -FilePath $report
"" | Tee-Object -FilePath $report -Append
foreach ($name in $selected) {
    "scenario: $name" | Tee-Object -FilePath $report -Append
    foreach ($server in $servers) {
        $set = $samples["$($server.Name)/$name"]
        if (-not $set -or $set.Count -eq 0) {
            "  {0,-10} no valid samples" -f $server.Name | Tee-Object -FilePath $report -Append
            continue
        }
        $sorted = $set | Sort-Object Rps
        $median = $sorted[[math]::Floor($set.Count / 2)]
        $best = $sorted[-1]
        "  {0,-10} median {1,10:N0} rps   best {2,10:N0} rps   p50 {3,-10} p99 {4,-10} cpu {5,4:N0}%  peak RSS {6,6:N1} MiB  n={7}" -f `
            $server.Name, $median.Rps, $best.Rps, $median.P50, $median.P99, $median.CpuPercent, $median.PeakMiB, $set.Count |
            Tee-Object -FilePath $report -Append
    }
    "" | Tee-Object -FilePath $report -Append
}
Write-Host "wrote $report"
