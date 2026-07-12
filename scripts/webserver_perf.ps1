param(
    [string]$BaseUrl = "http://127.0.0.1:11000",
    [int]$DurationSeconds = 8,
    [int[]]$ConcurrencyLevels = @(1, 10, 50),
    [string]$OutputCsv = "webserver_performance_results.csv"
)

$ErrorActionPreference = "Stop"

Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Threading.Tasks;

public sealed class LoadTestResult
{
    public string Url { get; set; }
    public int Concurrency { get; set; }
    public double DurationSeconds { get; set; }
    public long Requests { get; set; }
    public long Successful { get; set; }
    public long Failed { get; set; }
    public double RequestsPerSecond { get; set; }
    public double MeanMs { get; set; }
    public double P50Ms { get; set; }
    public double P95Ms { get; set; }
    public double P99Ms { get; set; }
    public double MaxMs { get; set; }
    public double ErrorRatePercent { get; set; }
    public string StatusCodes { get; set; }
}

public static class HttpLoadTester
{
    private static double Percentile(double[] sorted, double percentile)
    {
        if (sorted.Length == 0) return 0;
        int index = (int)Math.Ceiling(percentile * sorted.Length) - 1;
        return sorted[Math.Max(0, Math.Min(index, sorted.Length - 1))];
    }

    public static async Task<LoadTestResult> RunAsync(string url, int concurrency, int durationSeconds)
    {
        var handler = new SocketsHttpHandler
        {
            MaxConnectionsPerServer = int.MaxValue,
            AutomaticDecompression = DecompressionMethods.None,
            PooledConnectionLifetime = TimeSpan.FromMinutes(5)
        };
        using var client = new HttpClient(handler) { Timeout = TimeSpan.FromSeconds(10) };

        // Establish the connection and warm up JIT/routing before measurement.
        for (int i = 0; i < Math.Max(10, concurrency); i++)
        {
            using var warmup = await client.GetAsync(url, HttpCompletionOption.ResponseHeadersRead);
            await warmup.Content.ReadAsByteArrayAsync();
        }

        var latencies = new ConcurrentBag<double>();
        var statusCodes = new ConcurrentDictionary<int, long>();
        long successful = 0;
        long failed = 0;
        var wall = Stopwatch.StartNew();

        async Task Worker()
        {
            while (wall.Elapsed.TotalSeconds < durationSeconds)
            {
                var request = Stopwatch.StartNew();
                try
                {
                    using var response = await client.GetAsync(url, HttpCompletionOption.ResponseHeadersRead);
                    await response.Content.ReadAsByteArrayAsync();
                    request.Stop();
                    latencies.Add(request.Elapsed.TotalMilliseconds);
                    statusCodes.AddOrUpdate((int)response.StatusCode, 1, (_, count) => count + 1);
                    if (response.IsSuccessStatusCode)
                        System.Threading.Interlocked.Increment(ref successful);
                    else
                        System.Threading.Interlocked.Increment(ref failed);
                }
                catch
                {
                    request.Stop();
                    latencies.Add(request.Elapsed.TotalMilliseconds);
                    System.Threading.Interlocked.Increment(ref failed);
                }
            }
        }

        var workers = Enumerable.Range(0, concurrency).Select(_ => Worker()).ToArray();
        await Task.WhenAll(workers);
        wall.Stop();

        var values = latencies.ToArray();
        Array.Sort(values);
        long requests = successful + failed;
        return new LoadTestResult
        {
            Url = url,
            Concurrency = concurrency,
            DurationSeconds = wall.Elapsed.TotalSeconds,
            Requests = requests,
            Successful = successful,
            Failed = failed,
            RequestsPerSecond = requests / wall.Elapsed.TotalSeconds,
            MeanMs = values.Length == 0 ? 0 : values.Average(),
            P50Ms = Percentile(values, 0.50),
            P95Ms = Percentile(values, 0.95),
            P99Ms = Percentile(values, 0.99),
            MaxMs = values.Length == 0 ? 0 : values[values.Length - 1],
            ErrorRatePercent = requests == 0 ? 100 : failed * 100.0 / requests,
            StatusCodes = string.Join(";", statusCodes.OrderBy(pair => pair.Key).Select(pair => $"{pair.Key}:{pair.Value}"))
        };
    }
}
'@

$endpoints = @(
    @{ Name = "health"; Path = "/health" },
    @{ Name = "static_index"; Path = "/index.html" },
    @{ Name = "memory_api"; Path = "/api/home" },
    @{ Name = "database_api"; Path = "/api/temperature-humidity?username=perf-test" }
)

$health = Invoke-WebRequest -Uri "$BaseUrl/health" -UseBasicParsing -TimeoutSec 5
if ($health.StatusCode -ne 200) {
    throw "Webserver health check failed: HTTP $($health.StatusCode)"
}

$results = foreach ($endpoint in $endpoints) {
    foreach ($concurrency in $ConcurrencyLevels) {
        Write-Host "Testing $($endpoint.Name): concurrency=$concurrency, duration=${DurationSeconds}s"
        $result = [HttpLoadTester]::RunAsync(
            "$BaseUrl$($endpoint.Path)",
            $concurrency,
            $DurationSeconds
        ).GetAwaiter().GetResult()

        [pscustomobject]@{
            endpoint = $endpoint.Name
            concurrency = $result.Concurrency
            duration_s = [math]::Round($result.DurationSeconds, 3)
            requests = $result.Requests
            successful = $result.Successful
            failed = $result.Failed
            rps = [math]::Round($result.RequestsPerSecond, 2)
            mean_ms = [math]::Round($result.MeanMs, 3)
            p50_ms = [math]::Round($result.P50Ms, 3)
            p95_ms = [math]::Round($result.P95Ms, 3)
            p99_ms = [math]::Round($result.P99Ms, 3)
            max_ms = [math]::Round($result.MaxMs, 3)
            error_rate_percent = [math]::Round($result.ErrorRatePercent, 4)
            status_codes = $result.StatusCodes
        }
    }
}

$results | Export-Csv -Path $OutputCsv -NoTypeInformation -Encoding utf8
$results | Format-Table -AutoSize
Write-Host "Results written to $OutputCsv"
