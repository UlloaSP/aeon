# Aeon shell integration for PowerShell. Passed with -EncodedCommand and run
# before every first prompt: it loads the user's profiles, then Aeon's prompt.

# UTF-8 for text .NET writes, such as the marks and chevron below; the
# console's OEM code page would turn them into question marks.
[Console]::OutputEncoding = [Text.Encoding]::UTF8

# The user's profiles, in PowerShell's own order: their PATH, modules
# (Terminal-Icons), and PSReadLine settings. Aeon's prompt replaces theirs
# below; a broken profile is reported, not fatal. `[shell] profile = false`
# skips them.
$aeonProfiles = if ($env:AEON_NO_PROFILE -eq '1') { @() } else {
    @($PROFILE.AllUsersAllHosts, $PROFILE.AllUsersCurrentHost,
        $PROFILE.CurrentUserAllHosts, $PROFILE.CurrentUserCurrentHost)
}
foreach ($aeonProfile in $aeonProfiles) {
    if ($aeonProfile -and (Test-Path -LiteralPath $aeonProfile)) {
        try { . $aeonProfile } catch { Write-Warning "Aeon: ${aeonProfile}: $_" }
    }
}
Remove-Variable aeonProfile, aeonProfiles -ErrorAction SilentlyContinue

# PowerShell 7 shows folders on a blue background; unless a profile chose
# otherwise, show them as bold blue text.
if ($PSStyle -and $PSStyle.FileInfo.Directory -eq "$([char]27)[44;1m") {
    $PSStyle.FileInfo.Directory = "$([char]27)[1;34m"
}

# What you type, coloured by part and in bold: commands, arguments, strings,
# variables, with parameters and operators in gray. Unless a profile chose
# its own colours (PSReadLine's default command colour is 93).
if ((Get-Command Get-PSReadLineOption -ErrorAction SilentlyContinue) -and
    (Get-PSReadLineOption).CommandColor -eq "$([char]27)[93m") {
    $e = [char]27
    Set-PSReadLineOption -Colors @{
        Command = "$e[1;93m"; Default = "$e[1;97m"; Member = "$e[1;97m"; Number = "$e[1;97m"
        Parameter = "$e[1;38;5;245m"; Operator = "$e[38;5;245m"; Comment = "$e[38;5;242m"
        String = "$e[1;95m"; Variable = "$e[1;92m"; Keyword = "$e[1;94m"; Type = "$e[1;96m"
    }
    Remove-Variable e
}

# A line in PSReadLine's colours, for lines Aeon shows itself: a command from
# the launcher, and each statement of a line split into blocks.
function global:__aeon_colored([string] $line) {
    $options = if (Get-Command Get-PSReadLineOption -ErrorAction SilentlyContinue) { Get-PSReadLineOption }
    if (-not $options) { return $line }
    $tokens = $null
    $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseInput($line, [ref]$tokens, [ref]$errors)
    $out = ''
    $at = 0
    foreach ($token in $tokens) {
        $start = $token.Extent.StartOffset
        $end = $token.Extent.EndOffset
        if ($end -le $start) { continue }
        if ($start -gt $at) { $out += $line.Substring($at, $start - $at) }
        $flags = $token.TokenFlags
        $kind = [string]$token.Kind
        $color = if ($flags -band 'CommandName') { $options.CommandColor }
        elseif ($kind -eq 'Parameter') { $options.ParameterColor }
        elseif ($kind -like '*String*') { $options.StringColor }
        elseif ($kind -in 'Variable', 'SplattedVariable') { $options.VariableColor }
        elseif ($kind -eq 'Number') { $options.NumberColor }
        elseif ($kind -eq 'Comment') { $options.CommentColor }
        elseif ($flags -band 'Keyword') { $options.KeywordColor }
        elseif ($flags -band 'TypeName') { $options.TypeColor }
        elseif ($kind -in 'Pipe', 'Semi', 'Comma' -or
            ($flags -band 'BinaryOperator, UnaryOperator, AssignmentOperator')) { $options.OperatorColor }
        else { $options.DefaultTokenColor }
        $out += "$color$($line.Substring($start, $end - $start))$([char]27)[0m"
        $at = $end
    }
    if ($at -lt $line.Length) { $out += $line.Substring($at) }
    $out
}

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

# A line of several statements (`npm run build; npm test`) runs as one block
# per statement: the first runs at once and the rest wait here, each run as a
# line of its own after a quiet prompt naming it. So each is formatted on its
# own and `$?` means what it would. `&&` and `||` chains are one statement.
$global:AeonQueue = [System.Collections.Generic.Queue[string]]::new()

# Ctrl+C stops the whole line, as it would without the split. A stopped
# statement still reports success, so Ctrl+C itself is noted: by a compiled
# handler, since the console calls it where no PowerShell can run.
& {
    $x = [System.Linq.Expressions.Expression]
    $note = $x::Call(
        $x::Property($null, [AppDomain], 'CurrentDomain'),
        [AppDomain].GetMethod('SetData', [type[]]@([string], [object])),
        $x::Constant('aeon.cancel'),
        $x::Convert($x::Constant($true), [object]))
    $handler = $x::Lambda([ConsoleCancelEventHandler], $note,
        $x::Parameter([object], 'sender'), $x::Parameter([ConsoleCancelEventArgs], 'args'))
    [Console]::add_CancelKeyPress($handler.Compile())
}

function global:prompt {
    $ok = $?
    $code = $global:LASTEXITCODE
    $e = [char]27
    $done = ''
    if ($global:AeonRan) {
        $exit = if ($ok) { 0 } elseif ($code) { $code } else { 1 }
        $done = "$e]133;D;$exit$e\"
        $global:AeonRan = $false
        # A blank line between a command's output and the next prompt, but
        # not at the top of a cleared screen.
        if ($Host.UI.RawUI.CursorPosition.Y -gt 0) { $done += "`n" }
    }
    if ([AppDomain]::CurrentDomain.GetData('aeon.cancel')) {
        [AppDomain]::CurrentDomain.SetData('aeon.cancel', $null)
        $global:AeonQueue.Clear()
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
    $global:AeonChevron = "$e[1;${color}m$([char]0x276F)$e[0m"
    if ($global:AeonQueue.Count) {
        # The next statement of a split line, as if typed at this prompt.
        $next = $global:AeonQueue.Peek() -replace '\s*\r?\n\s*', ' '
        return "$done$e]133;A$e\$report$global:AeonChevron $e]133;B$e\$(__aeon_colored $next)`r`n"
    }
    "$done$e]133;A$e\$report$global:AeonChevron $e]133;B$e\"
}

function global:__aeon_statements([string] $line) {
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($line, [ref]$null, [ref]$errors)
    if ($errors -or $ast.ParamBlock -or $ast.BeginBlock -or $ast.ProcessBlock -or -not $ast.EndBlock) {
        return , @()
    }
    return , @($ast.EndBlock.Statements | ForEach-Object { $_.Extent.Text })
}

# Marks where `$line` starts running (C) and returns what to run now: the
# line as typed, or its first statement with the rest queued.
function global:__aeon_line([string] $line) {
    $global:AeonRan = $true
    $e = [char]27
    $statements = __aeon_statements $line
    if ($statements.Count -lt 2) {
        [Console]::Write("$e]133;C$e\")
        return $line
    }
    foreach ($statement in $statements[1..($statements.Count - 1)]) {
        $global:AeonQueue.Enqueue($statement)
    }
    # The typed line now says only its first statement: the rows it took
    # (after the two-cell prompt, or `>> ` on continued lines) are redrawn.
    $width = [Math]::Max(1, $Host.UI.RawUI.BufferSize.Width)
    $rows = 0
    $lead = 2
    foreach ($part in $line -split '\r?\n') {
        $rows += [Math]::Max(1, [Math]::Ceiling(($lead + $part.Length) / $width))
        $lead = 3
    }
    $shown = $statements[0] -replace '\s*\r?\n\s*', ' '
    [Console]::Write("$e[${rows}F$e[0J$global:AeonChevron $(__aeon_colored $shown)`r`n")
    [Console]::Write("$e]133;C;cmdline_url=$([Uri]::EscapeDataString($statements[0]))$e\")
    $statements[0]
}

# PSReadLine reads the line; the moment it returns is when the command
# starts running. A queued statement is returned without reading.
if (Test-Path Function:\PSConsoleHostReadLine) {
    $global:AeonReadLine = ${function:PSConsoleHostReadLine}
    function global:PSConsoleHostReadLine {
        if ($global:AeonQueue.Count) {
            $next = $global:AeonQueue.Dequeue()
            $global:AeonRan = $true
            $e = [char]27
            [Console]::Write("$e]133;C;cmdline_url=$([Uri]::EscapeDataString($next))$e\")
            return $next
        }
        __aeon_line (& $global:AeonReadLine)
    }
}
