# install-key.ps1 - executed on a Windows target by sshelper, then deletes itself.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File install-key.ps1 <uploaded .pub file>
#
# * Always writes %USERPROFILE%\.ssh\authorized_keys.
# * If the account is an administrator and sshd_config contains the default
#   "Match Group administrators" block, also writes
#   %ProgramData%\ssh\administrators_authorized_keys - in that case it is the
#   only file sshd reads for administrators.
# * Files are rewritten as UTF-8 without BOM with LF line endings; existing
#   UTF-16 files (e.g. made by "echo key >> authorized_keys" in Windows
#   PowerShell) are converted. ACLs are reset to what sshd accepts.
# * The uploaded temp files (this script and the .pub) are always removed.
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads BOM-less scripts with
# the ANSI code page, and the output is shown in the caller's console.
param(
    [Parameter(Mandatory = $true)][string]$PubFile
)

$ErrorActionPreference = 'Stop'
$self = $MyInvocation.MyCommand.Path
$pubPath = if ([IO.Path]::IsPathRooted($PubFile)) { $PubFile } else { Join-Path (Split-Path -Parent $self) $PubFile }

function Say([string]$Message) { [Console]::Out.WriteLine("[remote] $Message") }

# Returns $true when the key was already present.
function Add-AuthorizedKey([string]$Path, [string]$Key, [string]$Blob) {
    $dir = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

    $lines = New-Object System.Collections.Generic.List[string]
    if (Test-Path -LiteralPath $Path) {
        # ReadAllText honours UTF-8 / UTF-16 BOMs, so broken files are recovered here.
        foreach ($l in ([IO.File]::ReadAllText($Path).TrimStart([char]0xFEFF) -split "`r?`n")) {
            if ($l.Trim()) { $lines.Add($l.TrimEnd()) }
        }
    }

    $present = $false
    foreach ($l in $lines) { if ($l.Contains($Blob)) { $present = $true; break } }
    if (-not $present) { $lines.Add($Key) }

    [IO.File]::WriteAllText($Path, (($lines -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding($false)))
    return $present
}

# Drop inherited and foreign ACEs, grant Full Control to the given SIDs only.
function Set-StrictAcl([string]$Path, [string[]]$Sids) {
    & icacls.exe $Path /reset | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls /reset failed for $Path" }
    $icaclsArgs = @($Path, '/inheritance:r', '/grant:r') + @($Sids | ForEach-Object { "*${_}:F" })
    & icacls.exe @icaclsArgs | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls /grant failed for $Path" }
}

$exitCode = 0
try {
    if (-not (Test-Path -LiteralPath $pubPath -PathType Leaf)) { throw "uploaded public key not found: $pubPath" }

    # Same as tr -d '\r' on Linux, plus BOM removal; keep the first non-empty line.
    $raw = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($pubPath))
    $key = $null
    foreach ($l in (($raw.TrimStart([char]0xFEFF) -replace "`r", '') -split "`n")) {
        if ($l.Trim()) { $key = $l.Trim(); break }
    }
    if (-not $key -or $key -notmatch '^(ssh-|ecdsa-|sk-)') { throw 'uploaded file does not look like an OpenSSH public key' }
    $blob = ($key -split '\s+')[1]
    if (-not $blob) { throw 'public key has no key data' }

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $isAdmin = (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    $systemSid = 'S-1-5-18'
    $adminsSid = 'S-1-5-32-544'

    $userFile = Join-Path $env:USERPROFILE '.ssh\authorized_keys'
    $had = Add-AuthorizedKey $userFile $key $blob
    Set-StrictAcl $userFile @($identity.User.Value, $systemSid, $adminsSid)
    Say ('{0}: {1}' -f $userFile, $(if ($had) { 'key already present' } else { 'key added' }))

    if ($isAdmin) {
        $sshdConfig = Join-Path $env:ProgramData 'ssh\sshd_config'
        $adminBlock = (Test-Path -LiteralPath $sshdConfig) -and
            [bool](Select-String -LiteralPath $sshdConfig -Pattern '^\s*Match\s+Group\s+"?administrators\b' -Quiet)
        if ($adminBlock) {
            $adminFile = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys'
            $had = Add-AuthorizedKey $adminFile $key $blob
            Set-StrictAcl $adminFile @($adminsSid, $systemSid)
            Say ('{0}: {1} (sshd uses this file for Administrators)' -f $adminFile, $(if ($had) { 'key already present' } else { 'key added' }))
        }
    }
    Say 'done'
} catch {
    [Console]::Error.WriteLine("[remote] ERROR: $($_.Exception.Message)")
    $exitCode = 1
} finally {
    Remove-Item -LiteralPath $pubPath, $self -Force -ErrorAction SilentlyContinue
}
exit $exitCode
