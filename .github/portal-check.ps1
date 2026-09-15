# Drive the portal the way a person does: browse to a repository in the home
# directory, index it from the web view, then search what landed.
#
# This runs under PowerShell on all three operating systems, and that is the
# point rather than a convenience. `smoke.sh` runs under bash, which on Windows
# is Git Bash — and Git Bash sets HOME, which Windows itself does not. Every
# semlith lookup of HOME therefore succeeded on the Windows runner and would
# fail for a person who launched the daemon from PowerShell, cmd or a desktop
# shortcut. A harness that only ever runs under bash cannot see that class of
# bug at all.
#
# So on Windows the check removes HOME first, to hold the environment a real
# user has. Everything the script needs for its own bookkeeping comes from
# PowerShell's $HOME automatic variable, which is derived from USERPROFILE and
# is set whether or not the environment variable is.

$ErrorActionPreference = 'Continue'
$script:fails = 0

function Step {
    param([string]$Name, [scriptblock]$Body)
    Write-Host ""
    Write-Host "### $Name"
    try {
        & $Body
        Write-Host "--- ok: $Name"
    } catch {
        Write-Host "--- FAIL: $Name"
        Write-Host "    $($_.Exception.Message)"
        $script:fails++
    }
}

function Fail { param([string]$Message) throw $Message }

# ------------------------------------------------------------------ environment

Write-Host "### environment as semlith will see it"
Write-Host "PowerShell     : $($PSVersionTable.PSVersion)"
Write-Host "platform       : $([System.Environment]::OSVersion.Platform) / $($PSVersionTable.OS)"
Write-Host "`$HOME (shell)  : $HOME"
Write-Host "HOME (env)     : $(if ($null -ne $env:HOME) { $env:HOME } else { '<unset>' })"
Write-Host "USERPROFILE    : $(if ($null -ne $env:USERPROFILE) { $env:USERPROFILE } else { '<unset>' })"
Write-Host "SEMLITH_HOME   : $(if ($null -ne $env:SEMLITH_HOME) { $env:SEMLITH_HOME } else { '<unset>' })"

if ($IsWindows) {
    if ($null -ne $env:HOME) {
        Write-Host "note: the runner set HOME; removing it, because a user's PowerShell has none"
        Remove-Item Env:HOME
    }
    # SEMLITH_HOME would paper over exactly the lookup under test.
    if ($null -ne $env:SEMLITH_HOME) {
        Write-Host "note: removing SEMLITH_HOME so the HOME fallback is the thing exercised"
        Remove-Item Env:SEMLITH_HOME
    }
}

# The binary must be found the same way a user's shell finds it.
$semlith = (Get-Command semlith -ErrorAction SilentlyContinue)
if ($null -eq $semlith) { Write-Host "semlith is not on PATH"; exit 1 }
Write-Host "semlith        : $($semlith.Source)"

# ------------------------------------------------------------------ a repository in the home directory

# A real corpus in a real place: the portal refuses to browse outside the home
# directory, and a checkout in a CI workspace is not in one. Cloning gives the
# .git and .gitignore an ordinary repository has.
$repo = Join-Path $HOME 'semlith-portal-check'
Step "clone a repository into the home directory" {
    if (Test-Path $repo) { Remove-Item -Recurse -Force $repo }
    # AbsoluteUri gives file:///D:/a/... on Windows and file:///home/... on
    # unix. Hand-building the URL gets the slash count wrong on one or the
    # other, and a bare local path makes git ignore --depth.
    $source = ([uri](Get-Item $PWD.Path).FullName).AbsoluteUri
    Write-Host "cloning $source"
    git clone --depth 1 --quiet $source $repo 2>&1 | Out-Host
    if (-not (Test-Path (Join-Path $repo 'src'))) { Fail "clone produced no src directory at $repo" }
    Write-Host "cloned to $repo"
}

# The one-liner that reproduces what a person hits first. Indexing from the
# native shell, with nothing pre-seeded, is the whole of the first-run path.
Step "semlith index from the native shell" {
    $out = & semlith index --quiet $repo 2>&1 | Out-String
    Write-Host $out.Trim()
    if ($LASTEXITCODE -ne 0) { Fail "semlith index exited $LASTEXITCODE" }
    if ($out -match 'neither SEMLITH_HOME nor HOME is set') {
        Fail "semlith cannot find a store home in this shell's environment"
    }
}

Step "the store did not land in the working directory" {
    if (Test-Path (Join-Path $PWD.Path '.semlith')) {
        Fail "a store was written into the working directory $($PWD.Path)"
    }
    $expected = Join-Path $HOME '.semlith'
    if (-not (Test-Path $expected)) { Fail "no store home at $expected" }
    Write-Host "store home: $expected"
}

Step "the model cache did not land in the working directory" {
    if (Test-Path (Join-Path $PWD.Path '.cache')) {
        Fail "model weights were cached into the working directory $($PWD.Path)"
    }
}

# ------------------------------------------------------------------ the daemon

$port = if ($env:PORTAL_PORT) { [int]$env:PORTAL_PORT } else { 7366 }
$outFile = Join-Path ([System.IO.Path]::GetTempPath()) 'semlith-portal-out.txt'
$errFile = Join-Path ([System.IO.Path]::GetTempPath()) 'semlith-portal-err.txt'
$token = $null
$proc = $null

Step "start the daemon" {
    Remove-Item -Force -ErrorAction SilentlyContinue $outFile, $errFile
    $script:proc = Start-Process -FilePath $semlith.Source `
        -ArgumentList 'start', '--port', "$port" `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile `
        -PassThru -NoNewWindow
    for ($i = 0; $i -lt 90; $i++) {
        Start-Sleep -Milliseconds 1000
        if (Test-Path $outFile) {
            $text = Get-Content $outFile -Raw
            if ($text -match 'token=([0-9a-f]+)') { $script:token = $Matches[1]; break }
        }
        if ($script:proc.HasExited) { break }
    }
    Get-Content $outFile -ErrorAction SilentlyContinue | Out-Host
    Get-Content $errFile -ErrorAction SilentlyContinue | Out-Host
    if ($null -eq $script:token) { Fail "the daemon printed no portal URL within 90s" }
}

function Api {
    param([string]$Path, [string]$Method = 'GET', $Body = $null)
    # Not $args: that is an automatic variable inside a function.
    $req = @{
        Uri                = "http://127.0.0.1:$port$Path"
        Method             = $Method
        Headers            = @{ 'Semlith-Token' = $script:token }
        SkipHttpErrorCheck = $true
        TimeoutSec         = 300
    }
    if ($null -ne $Body) {
        $req.Body = ($Body | ConvertTo-Json -Compress)
        $req.ContentType = 'application/json'
    }
    Invoke-WebRequest @req
}

Step "the portal page answers" {
    $r = Api '/'
    if ($r.StatusCode -ne 200) { Fail "GET / was $($r.StatusCode)" }
}

# The browse the web view does before it can offer anything to index. With no
# HOME this is where a Windows daemon starts refusing paths.
Step "browse the home directory" {
    $r = Api '/api/dirs'
    if ($r.StatusCode -ne 200) { Fail "GET /api/dirs was $($r.StatusCode): $($r.Content)" }
    $body = $r.Content | ConvertFrom-Json
    Write-Host "home the portal reports: $($body.home)"
    $real = [System.IO.Path]::GetFullPath($HOME).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $said = $body.home -replace '^\\\\\?\\', ''
    $said = $said.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    if ($said -ne $real) {
        Fail "the portal calls '$($body.home)' the home directory; this user's is '$real'"
    }
    if ($body.entries.Count -eq 0) { Fail "the home directory listed no entries" }
}

Step "browse to the cloned repository" {
    $r = Api "/api/dirs?path=$([uri]::EscapeDataString($repo))"
    if ($r.StatusCode -ne 200) { Fail "GET /api/dirs for the repo was $($r.StatusCode): $($r.Content)" }
    $body = $r.Content | ConvertFrom-Json
    if (-not ($body.entries.name -contains 'src')) {
        Fail "the repository listing has no src entry; got: $($body.entries.name -join ', ')"
    }
}

Step "index the repository from the portal" {
    $r = Api '/api/index' 'POST' @{ path = $repo }
    if ($r.StatusCode -ne 200) { Fail "POST /api/index was $($r.StatusCode): $($r.Content)" }
    $events = $r.Content -split "`n" | Where-Object { $_.Trim() }
    if ($events.Count -eq 0) { Fail "the index stream carried no events" }
    Write-Host "$($events.Count) event(s); last: $($events[-1])"
    $bad = $events | Where-Object { $_ -match '"error"' }
    if ($bad) { Fail "the index stream reported an error: $($bad[0])" }
}

Step "the store is open and holds the repository" {
    $r = Api '/api/stores'
    if ($r.StatusCode -ne 200) { Fail "GET /api/stores was $($r.StatusCode)" }
    if ($r.Content -notmatch 'semlith-portal-check') {
        Fail "no store names the indexed repository: $($r.Content)"
    }
}

Step "search from the portal returns usable locators" {
    $r = Api '/api/search?query=store+lock&k=5'
    if ($r.StatusCode -ne 200) { Fail "GET /api/search was $($r.StatusCode): $($r.Content)" }
    $body = $r.Content | ConvertFrom-Json
    if (-not $body.hits -or $body.hits.Count -eq 0) { Fail "search returned no hits" }
    Write-Host "first hit: $($body.hits[0].path)"
    $verbatim = $body.hits | Where-Object { $_.path -like '\\?\*' }
    if ($verbatim) {
        Fail "a hit path is a \\?\ verbatim path, which no editor will open: $($verbatim[0].path)"
    }
}

Step "stop the daemon" {
    if ($null -ne $script:proc -and -not $script:proc.HasExited) {
        Stop-Process -Id $script:proc.Id -Force
        $script:proc.WaitForExit(15000) | Out-Null
    }
    if (Test-Path $errFile) {
        $stderr = (Get-Content $errFile -Raw).Trim()
        if ($stderr) { Write-Host "daemon stderr: $stderr" }
    }
}

Write-Host ""
Write-Host "### $script:fails failure(s)"
exit $script:fails
