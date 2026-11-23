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
    # The context bar shows where you are, so the prompt is just the mark:
    # a bold chevron, green after success and red after a failure.
    $color = if ($ok) { 32 } else { 31 }
    "$report$e[1;${color}m$([char]0x276F)$e[0m "
}
