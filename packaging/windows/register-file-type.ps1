# Register this extracted release for the current user; no administrator needed.
$ErrorActionPreference = 'Stop'
$binary = Join-Path $PSScriptRoot 'fast-cutvid.exe'
if (!(Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw 'Keep this script beside fast-cutvid.exe before running it.'
}
$classes = 'HKCU:\Software\Classes'
$project = "$classes\fastCutVid.Project"
New-Item -Path "$classes\.fastcut" -Force | Out-Null
Set-Item -Path "$classes\.fastcut" -Value 'fastCutVid.Project'
New-Item -Path "$project\shell\open\command" -Force | Out-Null
Set-Item -Path $project -Value 'fastCutVid Project'
Set-Item -Path "$project\shell\open\command" -Value ('"' + $binary + '" "%1"')
New-Item -Path "$project\DefaultIcon" -Force | Out-Null
Set-Item -Path "$project\DefaultIcon" -Value ('"' + (Join-Path $PSScriptRoot 'fastCutVid.ico') + '"')
Write-Host 'Registered .fastcut projects. If prompted, choose fastCutVid in Open with. Run this script again if you move the release folder.'
