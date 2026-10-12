param(
    [string]$Executable = (Join-Path $PSScriptRoot '../target/debug/ascurra-harness.exe'),
    [string]$Output = (Join-Path $PSScriptRoot '../target/renderer-memory.json'),
    [int]$Runs = 3,
    [string[]]$Modes = @('gpu', 'desktop-gl', 'software')
)
$ErrorActionPreference = 'Stop'
$results = @()
foreach ($mode in $Modes) {
    for ($run = 1; $run -le $Runs; $run++) {
        $arguments = '--measure-renderer'
        if ($mode -eq 'desktop-gl') { $arguments += ' --desktop-gl' }
        $start = [System.Diagnostics.ProcessStartInfo]::new([System.IO.Path]::GetFullPath($Executable), $arguments)
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.Environment['SLINT_BACKEND'] = if ($mode -eq 'software') { 'winit-software' } else { 'winit-femtovg' }
        $process = [System.Diagnostics.Process]::Start($start)
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        $peak = 0L
        $private = 0L
        $samples = @()
        while (-not $process.HasExited) {
            $process.Refresh()
            $peak = [Math]::Max($peak, $process.WorkingSet64)
            $private = [Math]::Max($private, $process.PrivateMemorySize64)
            if ($clock.ElapsedMilliseconds -ge 900 -and $clock.ElapsedMilliseconds -le 1700) { $samples += $process.WorkingSet64 }
            Start-Sleep -Milliseconds 50
        }
        $results += [pscustomobject]@{
            mode = $mode
            run = $run
            workingSetMiB = [Math]::Round((($samples | Measure-Object -Average).Average / 1MB), 1)
            peakWorkingSetMiB = [Math]::Round($peak / 1MB, 1)
            peakPrivateMiB = [Math]::Round($private / 1MB, 1)
            exitCode = $process.ExitCode
            renderer = $process.StandardOutput.ReadToEnd().Trim()
            error = $process.StandardError.ReadToEnd().Trim()
        }
        $process.Dispose()
    }
}
$results | ConvertTo-Json | Set-Content -LiteralPath $Output -Encoding utf8
$results | Select-Object mode, run, workingSetMiB, peakPrivateMiB, exitCode | Format-Table -AutoSize
