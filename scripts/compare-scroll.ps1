param(
    [Parameter(Mandatory=$true)][string]$Baseline,
    [string]$Candidate = ''
)
$ErrorActionPreference = 'Stop'
$before = (& "$PSScriptRoot/summarize-scroll.ps1" -Path $Baseline | Out-String | ConvertFrom-Json)
$after = (& "$PSScriptRoot/summarize-scroll.ps1" -Path $Candidate | Out-String | ConvertFrom-Json)
if ($before.file -eq $after.file) { throw 'La captura candidata y la referencia son el mismo archivo.' }
function Change($old, $new) {
    if ($null -eq $old -or $null -eq $new -or [double]$old -le 0) { return $null }
    return [Math]::Round(100 * ([double]$new - [double]$old) / [double]$old, 2)
}
$results = @()
foreach ($original in $before.panes) {
    $candidatePane = $after.panes | Where-Object pane -eq $original.pane | Select-Object -First 1
    if (!$candidatePane) { continue }
    $results += [pscustomobject]@{
        pane = $original.pane
        samplesBefore = $original.measuredFrames
        samplesAfter = $candidatePane.measuredFrames
        renderP50BeforeMs = $original.renderCpuMs.p50
        renderP50AfterMs = $candidatePane.renderCpuMs.p50
        renderP50ChangePercent = Change $original.renderCpuMs.p50 $candidatePane.renderCpuMs.p50
        renderP95BeforeMs = $original.renderCpuMs.p95
        renderP95AfterMs = $candidatePane.renderCpuMs.p95
        renderP95ChangePercent = Change $original.renderCpuMs.p95 $candidatePane.renderCpuMs.p95
        wheelResponseP95BeforeMs = $original.wheelToRenderCompleteMs.p95
        wheelResponseP95AfterMs = $candidatePane.wheelToRenderCompleteMs.p95
        cpuOverBudgetBeforePercent = $original.cpuFramesOverMonitorBudgetPercent
        cpuOverBudgetAfterPercent = $candidatePane.cpuFramesOverMonitorBudgetPercent
        fileLineCountsBefore = $original.fileLineCounts
        fileLineCountsAfter = $candidatePane.fileLineCounts
        gitRowCountsBefore = $original.gitRowCounts
        gitRowCountsAfter = $candidatePane.gitRowCounts
    }
}
[pscustomobject]@{
    baseline = $before.file
    candidate = $after.file
    optimizationBefore = $before.optimizationLevel
    optimizationAfter = $after.optimizationLevel
    monitorHzBefore = $before.monitorHz
    monitorHzAfter = $after.monitorHz
    interpretation = 'Un cambio negativo es menor tiempo medido. Repetir con los mismos archivos, tamaño de ventana, paneles y movimientos; una diferencia aislada no prueba causalidad.'
    panes = $results
} | ConvertTo-Json -Depth 6
