param([string]$Path = '')
$ErrorActionPreference = 'Stop'
if (!$Path) {
    $data = if ($env:HARNESS_DATA_DIR) { $env:HARNESS_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'AscurraHarness' }
    $directory = Join-Path $data 'logs/performance'
    $latest = Get-ChildItem -LiteralPath $directory -File -Filter 'scroll-*.jsonl' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (!$latest) { throw 'Todavía no hay una captura de scroll.' }
    $Path = $latest.FullName
}
function Distribution($values) {
    $sorted = @($values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if (!$sorted.Count) { return $null }
    $result = [ordered]@{ count = $sorted.Count }
    foreach ($entry in @(@('p50',0.50),@('p95',0.95),@('p99',0.99))) {
        $index = [Math]::Max(0, [Math]::Ceiling($entry[1] * $sorted.Count) - 1)
        $result[$entry[0]] = [Math]::Round($sorted[$index], 3)
    }
    $result['max'] = [Math]::Round($sorted[-1], 3)
    return [pscustomobject]$result
}
$records = [System.Collections.Generic.List[object]]::new()
$metadata = $null
$count = 0
foreach ($line in [IO.File]::ReadLines([IO.Path]::GetFullPath($Path))) {
    if (++$count -gt 20000) { break }
    try { $row = $line | ConvertFrom-Json } catch { continue }
    if ($row.event -eq 'scroll_profile_started') { $metadata = $row }
    if ($row.event -eq 'scroll_frame') { $records.Add($row) }
}
$hz = if ($metadata.refresh_millihertz) { [double]$metadata.refresh_millihertz / 1000 } else { 144.0 }
$budget = 1000 / $hz
$panes = @()
foreach ($group in ($records | Group-Object pane)) {
    $rows = @($group.Group)
    $input = @($rows | Where-Object { $null -ne $_.wheel_to_render_complete_ms })
    $created = 0
    foreach ($row in $rows) { foreach ($name in @('file','git_changes','git_history','chat')) { $created += [int]$row.rows_created.$name } }
    $panes += [pscustomobject]@{
        pane = $group.Name
        measuredFrames = $rows.Count
        framesWithWheelInput = $input.Count
        renderCpuMs = Distribution @($rows | ForEach-Object { $_.render_cpu_ms })
        wheelToBeforeRenderMs = Distribution @($input | ForEach-Object { $_.wheel_to_before_render_ms })
        wheelToRenderCompleteMs = Distribution @($input | ForEach-Object { $_.wheel_to_render_complete_ms })
        frameIntervalMs = Distribution @($rows | ForEach-Object { $_.frame_interval_ms })
        rowsCreated = $created
        fileLineCounts = @($rows.file_lines | Where-Object { $null -ne $_ } | Sort-Object -Unique)
        gitRowCounts = @($rows.git_rows | Where-Object { $null -ne $_ } | Sort-Object -Unique)
        chatMessageCounts = @($rows.chat_messages | Where-Object { $null -ne $_ } | Sort-Object -Unique)
        cpuFramesOverMonitorBudgetPercent = [Math]::Round(100 * @($rows | Where-Object { $_.render_cpu_ms -gt $budget }).Count / [Math]::Max(1,$rows.Count), 2)
        droppedRecords = ($rows | Measure-Object dropped_records -Maximum).Maximum
    }
}
[pscustomobject]@{
    file = [IO.Path]::GetFullPath($Path)
    debugBuild = $metadata.debug_build
    optimizationLevel = $metadata.opt_level
    buildProfile = $metadata.build_profile
    monitorHz = $hz
    monitorHzReported = [bool]$metadata.refresh_millihertz
    monitorFrameBudgetMs = [Math]::Round($budget, 3)
    interpretation = 'Tiempos de CPU y respuesta del bucle UI; no son tiempos GPU ni FPS de presentación. Los intervalos también dependen de la frecuencia de entrada del ratón.'
    truncatedAnalysis = $count -gt 20000
    panes = $panes
} | ConvertTo-Json -Depth 7
