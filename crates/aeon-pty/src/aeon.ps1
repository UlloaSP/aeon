# Aeon shell integration for PowerShell. Passed with -EncodedCommand in place
# of the user's profile, so it runs before every first prompt: keep it fast.

# UTF-8 for text .NET writes, such as the marks and chevron below; the
# console's OEM code page would turn them into question marks.
[Console]::OutputEncoding = [Text.Encoding]::UTF8

# Aeon keeps its own history, apart from other terminals' ConsoleHost_history.
$aeonHistory = Join-Path $env:APPDATA 'aeon\history'
$null = New-Item -ItemType Directory -Force -Path $aeonHistory
if (Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue) {
    $aeonEdition = if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh' } else { 'powershell' }
    Set-PSReadLineOption -HistorySavePath (Join-Path $aeonHistory "$aeonEdition.txt")
    Remove-Variable aeonEdition
}
Remove-Variable aeonHistory

# OSC 133 marks each command for Aeon's command blocks: A where the prompt
# starts, B where typing starts, C when the command runs, D;exit when it ends.
$global:AeonRan = $false

function global:prompt {
    $ok = $?
    $code = $global:LASTEXITCODE
    $e = [char]27
    $done = ''
    if ($global:AeonRan) {
        $exit = if ($ok) { 0 } elseif ($code) { $code } else { 1 }
        $done = "$e]133;D;$exit$e\"
        $global:AeonRan = $false
    }
    $location = $executionContext.SessionState.Path.CurrentLocation
    $report = ''
    if ($location.Provider.Name -eq 'FileSystem') {
        # OSC 9;9 reports the working directory to Aeon's sidebar and header.
        $report = "$e]9;9;`"$($location.ProviderPath)`"$e\"
    }
    # The header shows where you are, so the prompt is just the mark: a bold
    # chevron, green after success and red after a failure.
    $color = if ($ok) { 32 } else { 31 }
    "$done$e]133;A$e\$report$e[1;${color}m$([char]0x276F)$e[0m $e]133;B$e\"
}

# PSReadLine reads the line; marking C right after it returns is the moment
# the command starts running.
if (Test-Path Function:\PSConsoleHostReadLine) {
    $global:AeonReadLine = ${function:PSConsoleHostReadLine}
    function global:PSConsoleHostReadLine {
        $line = & $global:AeonReadLine
        $global:AeonRan = $true
        [Console]::Write("$([char]27)]133;C$([char]27)\")
        $line
    }
}
