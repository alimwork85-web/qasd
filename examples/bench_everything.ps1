# Qasd vs Everything: the same name queries through each one's command-line
# client (qasd.exe and es.exe), end to end, process start included.
# Both daemons must be running with their index loaded; both asked for at
# most 50 results (qasd.exe prints its best 50). Prints the median of 20 timed runs per query.
#
#   .\examples\bench_everything.ps1 -Es C:\path\to\es.exe
param(
    [Parameter(Mandatory)] [string] $Es,
    [string] $Qasd = "$env:LOCALAPPDATA\Qasd\bin\qasd.exe",
    [int] $Runs = 20
)
$queries = 'notepad', 'readme', 'config', 'invoice', 'setup.exe', 'chrome', 'report 2024', 'qasd'

function Median([double[]] $v) { $s = $v | Sort-Object; $s[[int]($s.Count / 2)] }
function Time([scriptblock] $run) {
    1..3 | ForEach-Object { & $run | Out-Null }
    Median (1..$Runs | ForEach-Object { (Measure-Command { & $run | Out-Null }).TotalMilliseconds })
}

$rows = foreach ($q in $queries) {
    $words = $q -split ' '
    [pscustomobject]@{
        query      = $q
        qasd_ms    = [math]::Round((Time { & $Qasd @words }), 1)
        es_ms      = [math]::Round((Time { & $Es -n 50 @words }), 1)
        qasd_hits  = @(& $Qasd @words).Count
        es_hits    = @(& $Es -n 50 @words).Count
    }
}
$rows | Format-Table -AutoSize
"median of medians: qasd {0} ms, everything {1} ms" -f (Median $rows.qasd_ms), (Median $rows.es_ms)
