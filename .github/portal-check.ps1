# Every portal route, plus the environment checks the CLI harness cannot make.
#
# This runs under PowerShell on all three operating systems, and that is the
# point rather than a convenience. `smoke.sh` runs under bash, which on Windows
# is Git Bash, and Git Bash sets HOME, which Windows itself does not. Every
# semlith lookup of HOME therefore resolved on the Windows runner and none of
# them resolves for a person who launched the daemon from PowerShell, cmd or a
# desktop shortcut. A harness that only ever runs under bash cannot see that
# class of bug at all.
#
# The Windows runner does not set HOME either, so what this script sees is a
# user's environment and not a staged one; the removal below is a no-op today
# and a guard if a future runner image adds it. Everything the script needs for
# its own bookkeeping comes from PowerShell's $HOME automatic variable, which is
# derived from USERPROFILE and is set whether or not the variable is.
#
# SEMLITH_HOME is deliberately NOT set here. `smoke.sh` pins it so one
# unresolved bug cannot mask its other seventy checks; this file leaves it alone
# so the native resolution is the thing under test.

$ErrorActionPreference = 'Continue'
$script:passes = 0
$script:fails = 0
$script:xfails = 0
$script:xpasses = 0
$script:skips = 0
$script:attention = @()

$knownFile = if ($env:KNOWN_FAILURES) { $env:KNOWN_FAILURES } else { Join-Path $PSScriptRoot 'known-failures.txt' }
$platform = if ($IsWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
$family = if ($IsWindows) { 'windows' } else { 'unix' }

# id -> issue number, for the rows that apply to this platform.
$script:known = @{}
if (Test-Path $knownFile) {
    foreach ($line in Get-Content $knownFile) {
        if ($line -match '^\s*#' -or $line -match '^\s*$') { continue }
        $f = $line -split '\s+' | Where-Object { $_ }
        if ($f.Count -lt 3) { continue }
        if ($f[1] -in @('all', $platform, $family)) { $script:known[$f[0]] = $f[2] }
    }
}

# A check asserts a property and nothing else decides its fate: a known failure
# does not fail the job, an unknown one does, and a known one that starts
# passing does too, so a fix cannot land without this file being updated.
function Check {
    param([string]$Id, [string]$Desc, [scriptblock]$Body, [bool]$When = $true)
    if (-not $When) {
        $script:skips++
        '{0,-8} {1,-38} {2}' -f 'skip', $Id, 'a precondition did not hold' | Write-Host
        return
    }
    $ok = $true
    $detail = ''
    try { $out = & $Body 2>&1 | Out-String; if ($out) { $detail = $out } }
    catch { $ok = $false; $detail = $_.Exception.Message }
    $isKnown = $script:known.ContainsKey($Id)
    if ($ok -and -not $isKnown) {
        $script:passes++
        '{0,-8} {1,-38} {2}' -f 'ok', $Id, $Desc | Write-Host
    } elseif (-not $ok -and $isKnown) {
        $script:xfails++
        '{0,-8} {1,-38} {2} (#{3})' -f 'xfail', $Id, $Desc, $script:known[$Id] | Write-Host
    } elseif ($ok -and $isKnown) {
        $script:xpasses++
        $script:attention += $Id
        '{0,-8} {1,-38} {2}' -f 'XPASS', $Id, $Desc | Write-Host
        "         #$($script:known[$Id]) is fixed. Remove this id from known-failures.txt." | Write-Host
    } else {
        $script:fails++
        $script:attention += $Id
        '{0,-8} {1,-38} {2}' -f 'FAIL', $Id, $Desc | Write-Host
        ($detail -split "`n" | Select-Object -First 12) | ForEach-Object { "         $_" | Write-Host }
    }
}

function Fail { param([string]$Message) throw $Message }

# ------------------------------------------------------------------ environment

Write-Host "PowerShell  : $($PSVersionTable.PSVersion)"
Write-Host "platform    : $platform / $($PSVersionTable.OS)"
Write-Host "`$HOME       : $HOME"
Write-Host "HOME (env)  : $(if ($null -ne $env:HOME) { $env:HOME } else { '<unset>' })"
Write-Host "USERPROFILE : $(if ($null -ne $env:USERPROFILE) { $env:USERPROFILE } else { '<unset>' })"
Write-Host "SEMLITH_HOME: $(if ($null -ne $env:SEMLITH_HOME) { $env:SEMLITH_HOME } else { '<unset>' })"

if ($IsWindows) {
    if ($null -ne $env:HOME) {
        Write-Host "note: the runner set HOME; removing it, because a user's PowerShell has none"
        Remove-Item Env:HOME
    }
    if ($null -ne $env:SEMLITH_HOME) {
        Write-Host "note: removing SEMLITH_HOME so the HOME fallback is the thing under test"
        Remove-Item Env:SEMLITH_HOME
    }
}

$semlith = Get-Command semlith -ErrorAction SilentlyContinue
if ($null -eq $semlith) { Write-Host "semlith is not on PATH"; exit 1 }
Write-Host "semlith     : $($semlith.Source)"
Write-Host ""

# ------------------------------------------------------------------ the corpus

$repo = Join-Path $HOME 'semlith-portal-check'
$script:repoReady = $false
Check 'portal/env/corpus' 'a corpus is prepared in the home directory' {
    if (Test-Path $repo) { Remove-Item -Recurse -Force $repo }
    # A file:// URL so --depth is honoured. .NET recognises only a Windows path
    # as a file URI and returns an empty AbsoluteUri for a unix one, so unix
    # gets the prefix directly: "file://" + "/home/..." is already three slashes.
    $source = if ($IsWindows) { ([uri]$PWD.Path).AbsoluteUri } else { "file://$($PWD.Path)" }
    git clone --depth 1 --quiet $source $repo 2>&1 | Out-Null
    if (-not (Test-Path (Join-Path $repo 'src'))) {
        # git is not what this harness tests. A copy is as good a corpus, and
        # losing forty checks because a clone failed is the worse outcome.
        Write-Host "note: the clone produced nothing; copying the checkout instead"
        New-Item -ItemType Directory -Force -Path $repo | Out-Null
        Copy-Item -Recurse -Force (Join-Path $PWD.Path 'src') (Join-Path $repo 'src')
        foreach ($f in @('README.md', 'CHANGELOG.md')) {
            $src = Join-Path $PWD.Path $f
            if (Test-Path $src) { Copy-Item -Force $src $repo }
        }
    }
    if (-not (Test-Path (Join-Path $repo 'src'))) { Fail "no corpus at $repo" }
    $script:repoReady = $true
}

$script:nativeIndex = $false
Check 'portal/env/native-shell-index' 'index works in the native shell' -When $script:repoReady {
    $out = & semlith index --quiet $repo 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { Fail "semlith index exited $LASTEXITCODE`n$($out.Trim())" }
    if ($out -match 'does not know where') {
        Fail "no store home in this shell's environment`n$($out.Trim())"
    }
    $script:nativeIndex = $true
}

# No SEMLITH_HOME fallback from here. It existed because the native lookup
# failed on Windows and every later check that needs a store would otherwise
# have been skipped -- 22 of 34 in the first full run. 0.17.1 resolves the home
# from USERPROFILE, so the native lookup is the thing that provides the store
# and there is nothing to stand in for it (#82).

Check 'portal/env/no-store-in-cwd' 'no store is written to the working directory' {
    if (Test-Path (Join-Path $PWD.Path '.semlith')) {
        Fail "a store was written into $($PWD.Path)"
    }
    if (-not (Test-Path (Join-Path $HOME '.semlith'))) {
        Fail "no store home at $(Join-Path $HOME '.semlith')"
    }
}

Check 'portal/env/no-cache-in-cwd' 'no model cache in the working directory' {
    if (Test-Path (Join-Path $PWD.Path '.cache')) {
        Fail "model weights were cached into $($PWD.Path)"
    }
}

# ------------------------------------------------------------- the deny-list

# A file inside a denied directory under the home directory must not be
# indexed, even when it is named explicitly. `denied` in src/filter.rs reads
# HOME to locate those directories, so on Windows, where HOME is unset, the
# whole directory rule is skipped and this is where that shows.
#
# `.kube` rather than `.ssh`: both are on the list and the probe is harmless
# either way, but a harness that writes into an ssh directory is one nobody
# will want to run twice. The probe file is uniquely named, the check refuses
# to touch anything that already exists, and it removes only what it created,
# so this is safe to run on a real machine and not just a runner.
#
# The file name matters. It must match none of DENIED_NAMES and must not begin
# with a dot, or it would be refused by a different rule and the check would
# pass without ever exercising the directory rule.
Check 'deny/denied-directory' 'a file in a denied directory is not indexed' {
    $denyDir = Join-Path $HOME '.kube'
    $probe = Join-Path $denyDir 'semlith-harness-probe.txt'
    $store = Join-Path ([System.IO.Path]::GetTempPath()) 'semlith-deny-probe-store'
    if (Test-Path $probe) { Fail "$probe already exists; refusing to touch it" }
    $madeDir = -not (Test-Path $denyDir)
    if ($madeDir) { New-Item -ItemType Directory -Path $denyDir -Force | Out-Null }
    try {
        Set-Content -Path $probe -Encoding utf8 -Value @(
            'This file exists only so the harness can check that semlith refuses'
            'to index anything under a credentials directory. It holds nothing.'
        )
        if (Test-Path $store) { Remove-Item -Recurse -Force $store }
        $out = & semlith --store $store index $probe 2>&1 | Out-String
        $listed = & semlith --store $store files 2>&1 | Out-String
        if ($listed -match 'semlith-harness-probe') {
            Fail "a file under $denyDir was indexed, so the directory rule did not apply`n$($out.Trim())"
        }
    } finally {
        Remove-Item -Force $probe -ErrorAction SilentlyContinue
        if ($madeDir) { Remove-Item -Force $denyDir -ErrorAction SilentlyContinue }
        Remove-Item -Recurse -Force $store -ErrorAction SilentlyContinue
    }
}

# ------------------------------------------------------------------- the daemon

$port = if ($env:PORTAL_PORT) { [int]$env:PORTAL_PORT } else { 7366 }
$outFile = Join-Path ([System.IO.Path]::GetTempPath()) 'semlith-portal-out.txt'
$errFile = Join-Path ([System.IO.Path]::GetTempPath()) 'semlith-portal-err.txt'
$script:token = $null
$script:proc = $null
$script:up = $false

Check 'portal/daemon/starts' 'the daemon starts and prints a tokenised URL' {
    Remove-Item -Force -ErrorAction SilentlyContinue $outFile, $errFile
    $script:proc = Start-Process -FilePath $semlith.Source `
        -ArgumentList 'start', '--port', "$port" `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile `
        -PassThru -NoNewWindow
    for ($i = 0; $i -lt 120; $i++) {
        Start-Sleep -Milliseconds 1000
        if (Test-Path $outFile) {
            $text = Get-Content $outFile -Raw
            if ($text -match 'token=([0-9a-f]+)') { $script:token = $Matches[1]; break }
        }
        if ($script:proc.HasExited) { break }
    }
    if ($null -eq $script:token) {
        $o = (Get-Content $outFile -Raw -ErrorAction SilentlyContinue)
        $e = (Get-Content $errFile -Raw -ErrorAction SilentlyContinue)
        Fail "no portal URL within 120s`n$o`n$e"
    }
    $script:up = $true
}

function Api {
    param(
        [string]$Path,
        [string]$Method = 'GET',
        $Body = $null,
        # A switch rather than an empty-string sentinel: PowerShell coerces a
        # [string] parameter defaulting to $null into '', which silently turned
        # every authenticated request into an anonymous one.
        [switch]$NoToken,
        [string]$Token,
        [hashtable]$Extra = $null
    )
    $headers = @{}
    if (-not $NoToken) {
        $t = if ($PSBoundParameters.ContainsKey('Token')) { $Token } else { $script:token }
        if ($t) { $headers['Semlith-Token'] = $t }
    }
    if ($null -ne $Extra) { foreach ($k in $Extra.Keys) { $headers[$k] = $Extra[$k] } }
    $req = @{
        Uri                = "http://127.0.0.1:$port$Path"
        Method             = $Method
        Headers            = $headers
        SkipHttpErrorCheck = $true
        TimeoutSec         = 600
    }
    if ($null -ne $Body) {
        $req.Body = ($Body | ConvertTo-Json -Compress -Depth 6)
        $req.ContentType = 'application/json'
    }
    Invoke-WebRequest @req
}

# `Get <path>` returns the parsed body, failing the check if the status is not
# 200. Every read check goes through it so a 500 can never read as empty data.
function Get-Json {
    param([string]$Path)
    $r = Api $Path
    if ($r.StatusCode -ne 200) { Fail "GET $Path was $($r.StatusCode): $($r.Content)" }
    try { return $r.Content | ConvertFrom-Json }
    catch { Fail "GET $Path returned unparseable JSON: $($r.Content.Substring(0, [Math]::Min(200, $r.Content.Length)))" }
}

# ---------------------------------------------------------------------- access

Check 'portal/auth/page-is-public' 'the page needs no token' -When $script:up {
    $r = Api '/' -NoToken
    if ($r.StatusCode -ne 200) { Fail "GET / without a token was $($r.StatusCode)" }
}

Check 'portal/auth/asset-is-public' 'a static asset needs no token' -When $script:up {
    $r = Api '/style.css' -NoToken
    if ($r.StatusCode -ne 200) { Fail "GET /style.css without a token was $($r.StatusCode)" }
}

Check 'portal/auth/api-needs-token' 'an API route without a token is 401' -When $script:up {
    $r = Api '/api/stores' -NoToken
    if ($r.StatusCode -ne 401) { Fail "GET /api/stores without a token was $($r.StatusCode), not 401" }
}

Check 'portal/auth/bad-token' 'a wrong token is 401' -When $script:up {
    $r = Api '/api/stores' -Token ('0' * 64)
    if ($r.StatusCode -ne 401) { Fail "a wrong token was $($r.StatusCode), not 401" }
}

Check 'portal/auth/good-token' 'the printed token opens the API' -When $script:up {
    $r = Api '/api/stores'
    if ($r.StatusCode -ne 200) { Fail "GET /api/stores with the token was $($r.StatusCode)" }
}

Check 'portal/route/unknown-is-404' 'an unknown route is 404' -When $script:up {
    $r = Api '/api/no-such-route'
    if ($r.StatusCode -ne 404) { Fail "an unknown route was $($r.StatusCode), not 404" }
}

Check 'portal/route/wrong-method-is-405' 'a write route on GET is 405' -When $script:up {
    $r = Api '/api/index'
    if ($r.StatusCode -ne 405) { Fail "GET /api/index was $($r.StatusCode), not 405" }
}

# The corpus was indexed before the daemon started, so the store already has
# content and the read checks below do not depend on /api/index succeeding.
# Asking the store directly is what keeps a failure in one route from costing
# every later check its coverage.
$script:hasData = $false
if ($script:up) {
    try { $script:hasData = ((( Get-Json '/api/files').files | Measure-Object).Count -gt 0) }
    catch { Write-Host "note: the file list could not be read, so the store looks empty" }
}
if ($script:up -and -not $script:hasData -and $script:repoReady) {
    Write-Host "note: the store is empty; filling it from the CLI so the read checks still run"
    & semlith index --quiet $repo 2>&1 | Out-Null
    try { $script:hasData = ((( Get-Json '/api/files').files | Measure-Object).Count -gt 0) } catch { }
}

# ------------------------------------------------------------------- browsing

Check 'portal/dirs/home-is-user-home' 'the browser roots at the user home' -When $script:up {
    $body = Get-Json '/api/dirs'
    $real = [System.IO.Path]::GetFullPath($HOME).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $said = ($body.home -replace '^\\\\\?\\', '').TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    if ($said -ne $real) { Fail "the portal calls '$($body.home)' home; this user's is '$real'" }
    if ($body.entries.Count -eq 0) { Fail "the home directory listed no entries" }
}

Check 'portal/dirs/browse-repo' 'the cloned repository can be browsed' -When ($script:up -and $script:repoReady) {
    $body = Get-Json "/api/dirs?path=$([uri]::EscapeDataString($repo))"
    if (-not ($body.entries.name -contains 'src')) {
        Fail "no src entry; got: $($body.entries.name -join ', ')"
    }
}

Check 'portal/dirs/outside-home-refused' 'a path outside home is refused' -When $script:up {
    $outside = if ($IsWindows) { 'C:\Windows\System32' } else { '/etc' }
    $r = Api "/api/dirs?path=$([uri]::EscapeDataString($outside))"
    if ($r.StatusCode -eq 200) { Fail "$outside was browsable: $($r.Content.Substring(0, [Math]::Min(200, $r.Content.Length)))" }
}

Check 'portal/dirs/missing-refused' 'a path that does not resolve is refused' -When $script:up {
    $r = Api "/api/dirs?path=$([uri]::EscapeDataString((Join-Path $HOME 'no-such-directory-here')))"
    if ($r.StatusCode -eq 200) { Fail "a missing path answered 200" }
}

# -------------------------------------------------------------------- indexing

$script:indexed = $false

Check 'portal/index/no-path-is-400' 'an index with no path is 400' -When $script:up {
    $r = Api '/api/index' 'POST' @{ }
    if ($r.StatusCode -ne 400) { Fail "POST /api/index with no path was $($r.StatusCode), not 400" }
}

Check 'portal/index/stream' 'the web view indexes a repository' -When ($script:up -and $script:repoReady) {
    $r = Api '/api/index' 'POST' @{ path = $repo }
    if ($r.StatusCode -ne 200) { Fail "POST /api/index was $($r.StatusCode): $($r.Content)" }
    $events = $r.Content -split "`n" | Where-Object { $_.Trim() }
    if ($events.Count -eq 0) { Fail "the index stream carried no events" }
    $bad = $events | Where-Object { $_ -match '"error"' }
    if ($bad) { Fail "the stream reported an error: $($bad[0])" }
    $script:indexed = $true
}

Check 'portal/index/control' 'pause and resume are accepted' -When $script:hasData {
    foreach ($action in @('pause', 'resume')) {
        $r = Api '/api/index/control' 'POST' @{ action = $action }
        if ($r.StatusCode -ne 200) { Fail "$action was $($r.StatusCode): $($r.Content)" }
        $body = $r.Content | ConvertFrom-Json
        $want = ($action -eq 'pause')
        if ($body.paused -ne $want) { Fail "$action left paused=$($body.paused)" }
    }
}

Check 'portal/index/bad-action-400' 'an unknown control action is 400' -When $script:hasData {
    $r = Api '/api/index/control' 'POST' @{ action = 'somersault' }
    if ($r.StatusCode -ne 400) { Fail "an unknown action was $($r.StatusCode), not 400" }
}

# ----------------------------------------------------------------------- reads

Check 'portal/stores/holds-repo' 'the store list names the repository' -When $script:hasData {
    $body = Get-Json '/api/stores'
    if (($body | ConvertTo-Json -Depth 8) -notmatch 'semlith-portal-check') {
        Fail "no store names the indexed repository"
    }
}

Check 'portal/files/lists' 'the file list pages and reports a total' -When $script:hasData {
    $body = Get-Json '/api/files'
    if (($body.files | Measure-Object).Count -eq 0) { Fail "the first page is empty" }
    if ($null -eq $body.total -or $body.total -lt 1) { Fail "no usable total: $($body.total)" }
}

Check 'portal/files/offset' 'the file list offset moves the page' -When $script:hasData {
    $first = (Get-Json '/api/files?limit=5').files
    $next = (Get-Json '/api/files?limit=5&offset=5').files
    if (($first | Measure-Object).Count -eq 0 -or ($next | Measure-Object).Count -eq 0) {
        Fail "one of the two pages was empty"
    }
    if (($first | ConvertTo-Json -Depth 4) -eq ($next | ConvertTo-Json -Depth 4)) {
        Fail "offset=5 returned the same page"
    }
}

Check 'portal/files/bad-offset' 'an out-of-range offset is refused' -When $script:hasData {
    $r = Api '/api/files?offset=-1'
    if ($r.StatusCode -ne 400) { Fail "offset=-1 was $($r.StatusCode), not 400" }
}

Check 'portal/search/hits' 'search returns hits with locators' -When $script:hasData {
    $body = Get-Json '/api/search?query=store+lock&k=5'
    if (($body.hits | Measure-Object).Count -eq 0) { Fail "search returned no hits" }
    $h = $body.hits[0]
    foreach ($field in @('path', 'start_line', 'end_line', 'score')) {
        if ($null -eq $h.$field) { Fail "a hit has no $field" }
    }
}

Check 'portal/search/no-verbatim' 'no verbatim paths among the hits' -When $script:hasData {
    $body = Get-Json '/api/search?query=store+lock&k=5'
    $bad = $body.hits | Where-Object { $_.path -like '\\?\*' }
    if ($bad) { Fail "a hit path is a verbatim path: $($bad[0].path)" }
}

Check 'portal/search/k' 'k bounds the hit count' -When $script:hasData {
    $body = Get-Json '/api/search?query=store&k=2'
    $n = ($body.hits | Measure-Object).Count
    if ($n -gt 2) { Fail "k=2 returned $n hits" }
    if ($n -eq 0) { Fail "k=2 returned nothing" }
}

# A page is cut out of one search, so the comparison is against that search and
# not against a shallower one. Rank fusion combines each list's top-k, so the
# ranking for k=1 is not the first row of the ranking for k=2 -- asking for
# "offset=1 differs from k=1" would be asking the engine for a property it does
# not have, and passing it would mean the ranking had stopped depending on k.
Check 'portal/search/offset' 'offset moves the window' -When $script:hasData {
    $page = Get-Json '/api/search?query=store&k=2'
    $second = $page.hits[1]
    if ($null -eq $second) { Fail "k=2 returned fewer than two hits, so this proves nothing" }
    $body = Get-Json '/api/search?query=store&k=1&offset=1'
    $b = $body.hits[0]
    if ($null -eq $b) { Fail "offset=1 returned no hit" }
    if ($body.offset -ne 1) { Fail "the response does not echo the offset: $($body.offset)" }
    if ("$($b.path):$($b.start_line)" -ne "$($second.path):$($second.start_line)") {
        Fail "offset=1 gave $($b.path):$($b.start_line), not the second hit $($second.path):$($second.start_line)"
    }
}

Check 'portal/search/empty-query' 'an empty query is handled' -When $script:hasData {
    $r = Api '/api/search?query='
    if ($r.StatusCode -ge 500) { Fail "an empty query was $($r.StatusCode): $($r.Content)" }
}

Check 'portal/read/span' 'read returns one span' -When $script:hasData {
    $body = Get-Json "/api/read?target=$([uri]::EscapeDataString('src/main.rs:28-40'))"
    if (($body | ConvertTo-Json -Depth 6).Length -lt 10) { Fail "read returned nothing" }
}

Check 'portal/symbol' 'symbol answers' -When $script:hasData { Get-Json '/api/symbol?name=main' | Out-Null }
Check 'portal/neighbors' 'neighbors answers' -When $script:hasData { Get-Json '/api/neighbors?name=main' | Out-Null }
Check 'portal/path' 'path answers' -When $script:hasData { Get-Json '/api/path?from=main&to=open_store' | Out-Null }
Check 'portal/graph' 'graph answers' -When $script:hasData { Get-Json '/api/graph?name=main' | Out-Null }
Check 'portal/ledger' 'ledger answers' -When $script:hasData { Get-Json '/api/ledger' | Out-Null }

Check 'portal/pattern' 'a tree-sitter pattern matches' -When $script:hasData {
    $q = [uri]::EscapeDataString('(function_item name: (identifier) @name)')
    Get-Json "/api/pattern?query=$q&lang=rust" | Out-Null
}

Check 'portal/pattern/no-lang' 'a pattern without a language is refused' -When $script:hasData {
    $q = [uri]::EscapeDataString('(function_item)')
    $r = Api "/api/pattern?query=$q"
    if ($r.StatusCode -eq 200) { Fail "a pattern with no --lang answered 200" }
}

foreach ($route in @('models', 'languages', 'privacy', 'about', 'agents', 'setup')) {
    Check "portal/$route" "/api/$route answers" -When $script:up { Get-Json "/api/$route" | Out-Null }
}

# ---------------------------------------------------------------------- parity
# Every CLI answer and its portal equivalent come from one store, so they must
# not disagree. A portal that quietly reads a different store is worse than one
# that errors.

Check 'portal/parity/files' 'the portal and the CLI count the same files' -When $script:hasData {
    # `files` pages in fifteens; `total` is the whole count, which is what the
    # CLI listing is comparable to.
    $body = Get-Json '/api/files'
    $portalTotal = $body.total
    $cliCount = (& semlith files 2>$null | Where-Object { $_.Trim() } | Measure-Object).Count
    if ($null -eq $portalTotal) { Fail "/api/files reported no total" }
    if ($portalTotal -ne $cliCount) {
        Fail "the portal counts $portalTotal files and the CLI lists $cliCount"
    }
}

Check 'portal/parity/search' 'the portal and the CLI agree on the top hit' -When $script:hasData {
    $portalTop = (Get-Json '/api/search?query=store+lock&k=1').hits[0].path
    $cliJson = & semlith search "store lock" -k 1 --json 2>$null | Out-String
    $cliTop = ($cliJson | ConvertFrom-Json)[0].path
    if ($null -eq $portalTop -or $null -eq $cliTop) { Fail "one side returned no hit" }
    if ((Split-Path $portalTop -Leaf) -ne (Split-Path $cliTop -Leaf)) {
        Fail "the portal's top hit is $portalTop and the CLI's is $cliTop"
    }
}

# --------------------------------------------------------------------- writes

Check 'portal/add/url' 'add fetches one https URL' -When $script:hasData {
    $r = Api '/api/add' 'POST' @{ url = 'https://raw.githubusercontent.com/semlith/semlith/main/LICENSE' }
    if ($r.StatusCode -ne 200) { Fail "POST /api/add was $($r.StatusCode): $($r.Content)" }
}

Check 'portal/add/bad-scheme' 'a non-https URL is refused' -When $script:hasData {
    $r = Api '/api/add' 'POST' @{ url = 'ftp://example.com/x' }
    if ($r.StatusCode -eq 200) { Fail "an ftp URL was accepted" }
}

Check 'portal/forget/removes' 'forget drops a file from the store' -When $script:hasData {
    # The whole listing, not the default first page: the page is path-sorted and
    # a corpus whose first fifteen files are prose has no Rust file on it.
    $target = (Get-Json '/api/files?limit=500').files |
        Where-Object { "$_" -match '\.rs$' -or $_.path -match '\.rs$' } |
        Select-Object -First 1
    $path = if ($target -is [string]) { $target } else { $target.path }
    if (-not $path) { Fail "no Rust file in the listing to forget" }
    $r = Api '/api/forget' 'POST' @{ path = $path }
    if ($r.StatusCode -ne 200) { Fail "POST /api/forget was $($r.StatusCode): $($r.Content)" }
    $after = (Get-Json '/api/files').files
    $still = $after | Where-Object { ($_ -eq $path) -or ($_.path -eq $path) }
    if ($still) { Fail "still listed after forget: $path. The route said: $($r.Content)" }
}

Check 'portal/trust' 'trust accepts a directory' -When $script:up {
    $r = Api '/api/trust' 'POST' @{ path = $repo }
    if ($r.StatusCode -ge 500) { Fail "POST /api/trust was $($r.StatusCode): $($r.Content)" }
}

Check 'portal/trust/no-path-400' 'trust without a path is 400' -When $script:up {
    $r = Api '/api/trust' 'POST' @{ }
    if ($r.StatusCode -ne 400) { Fail "trust with no path was $($r.StatusCode), not 400" }
}

Check 'portal/adopt/no-path-400' 'adopt without a path is 400' -When $script:up {
    $r = Api '/api/adopt' 'POST' @{ }
    if ($r.StatusCode -ne 400) { Fail "adopt with no path was $($r.StatusCode), not 400" }
}

Check 'portal/root/missing-fields-400' 'repointing without fields is 400' -When $script:up {
    $r = Api '/api/root' 'POST' @{ store = 'only-one-field' }
    if ($r.StatusCode -ne 400) { Fail "root with one field was $($r.StatusCode), not 400" }
}

Check 'portal/endpoint/toggle' 'the MCP endpoint closes and reopens' -When $script:up {
    $closed = Api '/api/endpoint' 'POST' @{ open = $false }
    if ($closed.StatusCode -ne 200) { Fail "closing was $($closed.StatusCode): $($closed.Content)" }
    if (($closed.Content | ConvertFrom-Json).open -ne $false) { Fail "closing did not report open=false" }
    $opened = Api '/api/endpoint' 'POST' @{ open = $true }
    if ($opened.StatusCode -ne 200) { Fail "reopening was $($opened.StatusCode): $($opened.Content)" }
    if (($opened.Content | ConvertFrom-Json).open -ne $true) { Fail "reopening did not report open=true" }
}

Check 'portal/endpoint/missing-open-400' 'the endpoint route needs a value' -When $script:up {
    $r = Api '/api/endpoint' 'POST' @{ }
    if ($r.StatusCode -ne 400) { Fail "endpoint with no value was $($r.StatusCode), not 400" }
}

Check 'portal/upgrade/check' 'upgrade check answers without installing' -When $script:up {
    $r = Api '/api/upgrade' 'POST' @{ action = 'check' }
    # 502 is a network failure reaching the release feed, which is not a defect
    # in this route. Anything else that is not 200 is.
    if ($r.StatusCode -ne 200 -and $r.StatusCode -ne 502) {
        Fail "POST /api/upgrade check was $($r.StatusCode): $($r.Content)"
    }
}

Check 'portal/upgrade/bad-action-400' 'an unknown upgrade action is 400' -When $script:up {
    $r = Api '/api/upgrade' 'POST' @{ action = 'apply-everything' }
    if ($r.StatusCode -ne 400) { Fail "an unknown action was $($r.StatusCode), not 400" }
}

# -------------------------------------------------------------------- MCP

Check 'portal/mcp/tools-list' 'MCP over the portal route lists tools' -When $script:up {
    $r = Api '/api/mcp' 'POST' @{ jsonrpc = '2.0'; id = 1; method = 'tools/list'; params = @{ } }
    if ($r.StatusCode -ne 200) { Fail "POST /api/mcp was $($r.StatusCode): $($r.Content)" }
    if ($r.Content -notmatch '"tools"') { Fail "no tools in the answer: $($r.Content)" }
}

Check 'portal/mcp-http/needs-key' '/mcp without the agent key is 401' -When $script:up {
    $r = Api '/mcp' 'POST' @{ jsonrpc = '2.0'; id = 1; method = 'tools/list' } -NoToken
    if ($r.StatusCode -ne 401) { Fail "/mcp without a key was $($r.StatusCode), not 401" }
}

Check 'portal/mcp-http/with-key' '/mcp with the agent key lists tools' -When $script:up {
    $key = (& semlith key show 2>$null | Select-String -Pattern 'sml_[0-9a-f]+').Matches[0].Value
    if (-not $key) { Fail "could not read the agent key" }
    $r = Api '/mcp' 'POST' @{ jsonrpc = '2.0'; id = 1; method = 'tools/list'; params = @{ } } `
        -NoToken -Extra @{ Authorization = "Bearer $key" }
    if ($r.StatusCode -ne 200) { Fail "/mcp with the key was $($r.StatusCode): $($r.Content)" }
    if ($r.Content -notmatch '"tools"') { Fail "no tools in the answer" }
}

Check 'portal/agents/reveal' 'the agent key can be revealed' -When $script:up {
    $r = Api '/api/agents/reveal' 'POST' @{ }
    if ($r.StatusCode -ne 200) { Fail "reveal was $($r.StatusCode): $($r.Content)" }
    if (($r.Content | ConvertFrom-Json).key -notmatch '^sml_') { Fail "reveal returned no agent key" }
}

Check 'portal/key/rotate' 'the agent key rotates to a new value' -When $script:up {
    $before = ($(Api '/api/agents/reveal' 'POST' @{ }).Content | ConvertFrom-Json).key
    $r = Api '/api/key' 'POST' @{ }
    if ($r.StatusCode -ne 200) { Fail "POST /api/key was $($r.StatusCode): $($r.Content)" }
    $after = ($(Api '/api/agents/reveal' 'POST' @{ }).Content | ConvertFrom-Json).key
    if ($before -eq $after) { Fail "the agent key did not change: $before" }
}

# Last of the write checks: it replaces the token every later request needs.
Check 'portal/rotate/token' 'rotating the session token invalidates the old one' -When $script:up {
    $old = $script:token
    $r = Api '/api/rotate' 'POST' @{ }
    if ($r.StatusCode -ne 200) { Fail "POST /api/rotate was $($r.StatusCode): $($r.Content)" }
    $fresh = ($r.Content | ConvertFrom-Json).token
    if (-not $fresh) { Fail "rotate returned no token" }
    if ($fresh -eq $old) { Fail "rotate returned the same token" }
    $script:token = $fresh
    $stale = Api '/api/stores' -Token $old
    if ($stale.StatusCode -ne 401) { Fail "the old token still worked: $($stale.StatusCode)" }
    $current = Api '/api/stores'
    if ($current.StatusCode -ne 200) { Fail "the new token did not work: $($current.StatusCode)" }
}

# ------------------------------------------------------------------- teardown

Check 'portal/daemon/stops' 'the daemon stops cleanly' -When $script:up {
    if ($null -ne $script:proc -and -not $script:proc.HasExited) {
        Stop-Process -Id $script:proc.Id -Force
        $script:proc.WaitForExit(20000) | Out-Null
    }
    $stderr = if (Test-Path $errFile) { (Get-Content $errFile -Raw).Trim() } else { '' }
    if ($stderr -match 'panicked at') { Fail "the daemon panicked:`n$stderr" }
}

Check 'portal/daemon/releases-lock' 'the store lock is free afterwards' -When ($script:up -and $script:indexed) {
    $out = & semlith index --quiet $repo 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { Fail "indexing after shutdown exited $LASTEXITCODE`n$($out.Trim())" }
}

if (Test-Path $repo) { Remove-Item -Recurse -Force $repo -ErrorAction SilentlyContinue }

Write-Host ""
Write-Host "--------------------------------------------------------------"
'pass {0}   fail {1}   xfail {2}   XPASS {3}   skip {4}' -f `
    $script:passes, $script:fails, $script:xfails, $script:xpasses, $script:skips | Write-Host
if ($script:attention.Count -gt 0) {
    "needs attention: $($script:attention -join ' ')" | Write-Host
}
exit ($script:fails + $script:xpasses)
