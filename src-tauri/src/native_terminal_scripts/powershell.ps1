# NoriShell session-only PowerShell integration.
$global:__NoriShellNonce = '__NORISHELL_NONCE__'
$global:__NoriShellCaptureCommand = (__NORISHELL_CAPTURE_COMMAND__ -eq 1)
# A new Core nonce never inherits an old command's completion pairing.
$global:__NoriShellSequence = 0
$global:__NoriShellActiveId = $null

function global:__NoriShellEmit([string] $Phase, [string] $CommandId, [string] $Argument) {
  [Console]::Write("$([char]27)]6973;NoriShell;1;$global:__NoriShellNonce;$Phase;$CommandId;$Argument`a")
}

function global:__NoriShellStart([string] $Command) {
  if ([string]::IsNullOrWhiteSpace($Command) -or $Command.StartsWith('__NoriShell')) { return }
  $global:__NoriShellSequence += 1
  $global:__NoriShellActiveId = [string] $global:__NoriShellSequence
  $payload = '-'
  if ($global:__NoriShellCaptureCommand -and $Command -notmatch '^\s' -and [Text.Encoding]::UTF8.GetByteCount($Command) -le 16384) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Command))
    $payload = $encoded
  }
  __NoriShellEmit 'start' $global:__NoriShellActiveId $payload
}

if (-not (Get-Command Set-PSReadLineKeyHandler -ErrorAction SilentlyContinue)) {
  __NoriShellEmit 'unsupported' '0' 'psReadLineUnavailable'
  return
}

$__norishellEnter = Get-PSReadLineKeyHandler -Bound | Where-Object { $_.Key -eq 'Enter' } | Select-Object -First 1
if (-not $global:__NoriShellEnterInstalled -and ($null -eq $__norishellEnter -or $__norishellEnter.Function -ne 'AcceptLine')) {
  __NoriShellEmit 'unsupported' '0' 'enterBindingOwned'
  return
}

if (-not $global:__NoriShellPromptInstalled) {
  $global:__NoriShellOriginalPrompt = ${function:prompt}
  $global:__NoriShellPromptInstalled = $true
  function global:prompt {
    $noriSuccess = $?
    $noriActiveId = $global:__NoriShellActiveId
    # Preserve the pre-existing prompt's view of `$?`. `$LASTEXITCODE` can be
    # stale or unrelated to a cmdlet, so only success is a reliable zero.
    & $global:__NoriShellOriginalPrompt
    if ($null -ne $noriActiveId) {
      if ($noriSuccess) { $noriExit = '0' } else { $noriExit = 'unknown' }
      __NoriShellEmit 'end' $noriActiveId $noriExit
      $global:__NoriShellActiveId = $null
    }
    __NoriShellEmit 'prompt' '0' '-'
  }
}

Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
  param($key, $arg)
  $line = $null
  $cursor = 0
  [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref] $line, [ref] $cursor)
  __NoriShellStart $line
  [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine($key, $arg)
}
$global:__NoriShellEnterInstalled = $true
__NoriShellEmit 'ready' '0' '-'
