# Aeon shell integration for PowerShell. Passed with -EncodedCommand in place
# of the user's profile, so it runs before every first prompt: keep it fast.

# Aeon keeps its own history, apart from other terminals' ConsoleHost_history.
$aeonHistory = Join-Path $env:APPDATA 'aeon\history'
$null = New-Item -ItemType Directory -Force -Path $aeonHistory
if (Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue) {
    $aeonEdition = if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh' } else { 'powershell' }
    Set-PSReadLineOption -HistorySavePath (Join-Path $aeonHistory "$aeonEdition.txt")
    Remove-Variable aeonEdition
}
Remove-Variable aeonHistory

function global:prompt {
    $ok = $?
    $e = [char]27
    $location = $executionContext.SessionState.Path.CurrentLocation
    $report = ''
    if ($location.Provider.Name -eq 'FileSystem') {
        # OSC 9;9 reports the working directory to Aeon's context bar.
        $report = "$e]9;9;`"$($location.ProviderPath)`"$e\"
    }
    $leaf = Split-Path -Leaf $location.Path
    if (-not $leaf) { $leaf = $location.Path }
    $color = if ($ok) { 32 } else { 31 }
    "$report$e[34m$leaf$e[0m $e[${color}m$([char]0x203A)$e[0m "
}
