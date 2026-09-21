<#
.SYNOPSIS
    契约回放工具链：PostgreSQL scratch schema 的生命周期管理。

.DESCRIPTION
    所有契约测试只在 `compat_*` scratch schema 里建表/写数据/TRUNCATE/DROP。
    生产 `public` 下的 `app_*` / `store_*` / `commerce_*` 是现网数据，本脚本**永远不碰**：
    白名单只接受 `compat_` 前缀，`public` 会被明确拒绝。

    连接串从 `db/config.toml` 的 `[database].postgres_url` 读取（不硬编码口令）。
    目标 schema 通过连接串的 `?options=-csearch_path%3D<schema>` 生效——Rust 侧全部
    DDL/DML 都是非限定表名，因此换 search_path 即可完成隔离，零代码改动。

    DDL 由 `src/server/bootstrap/*.rs` 里的 `r#"..."#` 常量**现场抽取**（顺序与
    `bootstrap::init_database` 完全一致），不另抄一份；抽取后还会拒绝任何
    DROP/TRUNCATE/DELETE/ALTER 语句，防止把破坏性语句带进数据库。

.PARAMETER Init
    建 schema（幂等）。

.PARAMETER Reset
    drop + create，得到空的 scratch schema。

.PARAMETER Drop
    删除 schema（CASCADE）。

.PARAMETER Apply
    把从 Rust 源码抽取的 DDL 灌进该 schema。

.PARAMETER Backup
    pg_dump 整个现网库到 `db/backups/`（动 public 前的必备步骤）。

.PARAMETER List
    列出所有 `compat_*` schema 及其表数。

.PARAMETER Serve
    用该 schema 起一个 Rust 服务实例（影子 `/compat` 挂载在该端口上）。
    config.toml 要求 CWD 相对路径，且 ONNX 路径也是相对路径，所以这里在
    $env:TEMP 下造一个运行目录：写一份只改 postgres_url 的 config.toml，
    并把 `onnx/`、`static/` 以 junction 指回仓库。

.PARAMETER Port
    -Serve 的监听端口，默认 11000。

.PARAMETER Build
    -Serve 前先 `cargo build`。

.PARAMETER Stop
    停掉该 schema 的服务实例。

.EXAMPLE
    .\pg_env.ps1 -Reset compat_test
    .\pg_env.ps1 -Apply compat_test
    .\pg_env.ps1 -Serve compat_test -Build
    .\pg_env.ps1 -List
    .\pg_env.ps1 -Drop compat_test
#>
[CmdletBinding(DefaultParameterSetName = 'Init')]
param(
    [Parameter(ParameterSetName = 'Init', Mandatory = $true)]
    [string]$Init,

    [Parameter(ParameterSetName = 'Reset', Mandatory = $true)]
    [string]$Reset,

    [Parameter(ParameterSetName = 'Drop', Mandatory = $true)]
    [string]$Drop,

    [Parameter(ParameterSetName = 'Apply', Mandatory = $true)]
    [string]$Apply,

    [Parameter(ParameterSetName = 'Backup', Mandatory = $true)]
    [switch]$Backup,

    [Parameter(ParameterSetName = 'List', Mandatory = $true)]
    [switch]$List,

    [Parameter(ParameterSetName = 'Serve', Mandatory = $true)]
    [string]$Serve,

    [Parameter(ParameterSetName = 'Serve')]
    [int]$Port = 11000,

    [Parameter(ParameterSetName = 'Serve')]
    [switch]$Build,

    [Parameter(ParameterSetName = 'Serve')]
    [switch]$Stop
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --------------------------------------------------------------------------------------
# 路径与工具定位
# --------------------------------------------------------------------------------------

$DbRoot = Split-Path -Parent $PSScriptRoot
$RepoRoot = Split-Path -Parent $DbRoot
$ConfigPath = Join-Path $DbRoot 'config.toml'
$BootstrapDir = Join-Path $DbRoot 'src\server\bootstrap'
$BackupDir = Join-Path $DbRoot 'backups'
# 支持 CARGO_TARGET_DIR：并行开发时各工作流用**独占 target 目录**，避免共用 target 时
# 互相锁住 `ai-service.exe`（表现为 `link.exe` LNK1104）。默认仍是 <repo>/target。
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $DbRoot 'target' }
$ExePath = Join-Path $TargetDir 'debug\ai-service.exe'

function Resolve-PgTool([string]$Name) {
    $onPath = Get-Command $Name -ErrorAction Ignore
    if ($onPath) { return $onPath.Source }
    foreach ($dir in @($env:PG_BIN, 'D:\apps\pg\18\bin', 'C:\Program Files\PostgreSQL\18\bin')) {
        if (-not $dir) { continue }
        $candidate = Join-Path $dir "$Name.exe"
        if (Test-Path $candidate) { return $candidate }
    }
    throw "找不到 $Name：请把它放进 PATH，或设置 `$env:PG_BIN"
}

$Psql = Resolve-PgTool 'psql'

# --------------------------------------------------------------------------------------
# config.toml 读取 + 安全白名单
# --------------------------------------------------------------------------------------

function Get-BaseDsn {
    if (-not (Test-Path $ConfigPath)) { throw "找不到配置文件：$ConfigPath" }
    $text = Get-Content $ConfigPath -Raw
    $m = [regex]::Match($text, '(?m)^\s*postgres_url\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw "$ConfigPath 里没有 [database].postgres_url" }
    return $m.Groups[1].Value
}

function Assert-CompatSchema([string]$Schema) {
    if ($Schema -eq 'public') {
        throw "拒绝操作 public：现网 app_*/store_*/commerce_* 有真实数据，任何写/删/改都禁止。"
    }
    if ($Schema -notmatch '^compat_[a-z0-9_]+$') {
        throw "非法 schema 名 '$Schema'：只接受 ^compat_[a-z0-9_]+$（例如 compat_test）"
    }
}

function New-SchemaDsn([string]$BaseDsn, [string]$Schema) {
    # search_path 只放 scratch schema，不放 public —— 确保不可能误读现网表。
    $opt = 'options=-csearch_path%3D' + $Schema
    if ($BaseDsn -match '\?') { return "$BaseDsn&$opt" }
    return "$BaseDsn`?$opt"
}

function Invoke-Psql([string]$Dsn, [string[]]$Sql) {
    $args = @($Dsn, '-v', 'ON_ERROR_STOP=1', '-q')
    foreach ($stmt in $Sql) { $args += @('-c', $stmt) }
    $out = & $Psql @args 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "psql 失败（exit $LASTEXITCODE）:`n$($out -join "`n")"
    }
    return $out
}

function Get-DsnPassword([string]$Dsn) {
    $m = [regex]::Match($Dsn, '^[a-z]+://[^:/@]+:(?<pw>[^@]*)@')
    if ($m.Success) { $env:PGPASSWORD = $m.Groups['pw'].Value }
}

$BaseDsn = Get-BaseDsn
Get-DsnPassword $BaseDsn

# --------------------------------------------------------------------------------------
# DDL：从 Rust 源码现场抽取（顺序 = bootstrap::init_database）
# --------------------------------------------------------------------------------------

# 与 bootstrap.rs 里各 init_* 函数的调用顺序一致；改顺序会破坏外键依赖。
$DdlOrder = @(
    @{ File = 'legacy_tables.rs';   Const = 'DDL' }
    @{ File = 'core_tables.rs';     Const = 'DDL' }
    @{ File = 'trace_tables.rs';    Const = 'DDL' }
    @{ File = 'commerce_tables.rs'; Const = 'PRODUCT_DDL' }
    @{ File = 'trace_tables.rs';    Const = 'QUALITY_DDL' }
    @{ File = 'commerce_tables.rs'; Const = 'DDL' }
    @{ File = 'agent_tables.rs';    Const = 'DDL' }
    # 网页超集专用表（不参与契约），也是 `r#"..."#` 常量，必须一起抽，
    # 否则镜像 schema 会与 `init_database` 的实际建库结果脱节。
    @{ File = 'web_tables.rs';      Const = 'DDL' }
)

$Destructive = [regex]'(?im)^\s*(DROP|TRUNCATE|DELETE|GRANT|REVOKE|UPDATE|INSERT)\b'

# ALTER 不是一律禁止：`init_database` 会用**幂等的加法列**给契约表补超集列
# （`ALTER TABLE "user" ADD COLUMN IF NOT EXISTS is_admin ...`）。
# 只放行这一种形态；其余 ALTER（DROP COLUMN / TYPE / SET NOT NULL…）仍然拒绝。
$AdditiveAlter = [regex]'(?is)^ALTER\s+TABLE\s+"[^"]+"\s+ADD\s+COLUMN\s+IF\s+NOT\s+EXISTS\s+\S+\s+\S+.*$'

# `bootstrap.rs` 里以 `sqlx::query(r#"..."#)` 就地执行、且与上面 const 同属 `init_database` 的
# 加法语句。同样现场抽取、不另抄一份，避免与 `bootstrap.rs` 漂移。
$AdditivePattern = 'sqlx::query\(\s*r#"(ALTER\s+TABLE\s+"[^"]+"\s+ADD\s+COLUMN\s+IF\s+NOT\s+EXISTS[^"]*)"#'

function Get-BootstrapDdl {
    $statements = [System.Collections.Generic.List[string]]::new()

    foreach ($entry in $DdlOrder) {
        $path = Join-Path $BootstrapDir $entry.File
        if (-not (Test-Path $path)) { throw "找不到 DDL 源文件：$path" }
        $text = Get-Content $path -Raw

        $pattern = 'const\s+' + [regex]::Escape($entry.Const) +
                   '\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\n\];'
        $block = [regex]::Match($text, $pattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)
        if (-not $block.Success) {
            throw "$($entry.File) 里找不到 const $($entry.Const): &[&str]"
        }

        $found = [regex]::Matches(
            $block.Groups[1].Value,
            'r#"(.*?)"#',
            [System.Text.RegularExpressions.RegexOptions]::Singleline
        )
        if ($found.Count -eq 0) {
            throw "$($entry.File)::$($entry.Const) 是空的"
        }
        foreach ($f in $found) { $statements.Add($f.Groups[1].Value.Trim()) }
        Write-Host ("  {0,-20} {1,-14} {2,4}" -f $entry.File, $entry.Const, $found.Count)
    }

    $bootstrapPath = Join-Path $DbRoot 'src\server\bootstrap.rs'
    $bootstrapText = Get-Content $bootstrapPath -Raw
    $additive = [regex]::Matches(
        $bootstrapText,
        $AdditivePattern,
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )
    foreach ($m in $additive) { $statements.Add($m.Groups[1].Value.Trim()) }
    Write-Host ("  {0,-20} {1,-14} {2,4}" -f 'bootstrap.rs', 'additive ALTER', $additive.Count)

    foreach ($stmt in $statements) {
        if ($Destructive.IsMatch($stmt)) {
            throw "抽取到的 DDL 里出现破坏性语句，拒绝执行：`n$stmt"
        }
        if (($stmt -match '(?i)^\s*ALTER\b') -and (-not $AdditiveAlter.IsMatch($stmt))) {
            throw "只放行幂等的加法列 ALTER（ADD COLUMN IF NOT EXISTS），拒绝：`n$stmt"
        }
    }
    # 每个分组必须非空（见上面的 `$($entry.File)::$($entry.Const) 是空的`）已经能抓住
    # 「正则失效导致某个文件一条都抽不出来」。这里只再兜一层**总量**下限。
    #
    # ⚠️ 别把下限写成「当前条数 + 一点余量」那种魔法数字：它会被**合法的 DDL 退役**打死。
    # 实证：S5/G2 删掉 9 张旧表后总量 91 → 82，而当时的阈值是 90，于是 `-Apply` 直接报
    # 「抽取到的语句只有 82 条…拒绝执行」，把 VERIFICATION.md 的第 2 步整条堵死。
    # 现在的下限 60 是按**契约层的量级**定的（30 张契约表约 66 条，且契约层是冻结的、
    # 不该变少），所以它只在「契约表整块抽丢」时才会触发。
    if ($statements.Count -lt 60) {
        throw "抽取到的语句只有 $($statements.Count) 条，明显不完整（期望 >= 60，契约层约 66 条），拒绝执行"
    }
    return $statements
}

# --------------------------------------------------------------------------------------
# 动作
# --------------------------------------------------------------------------------------

function Invoke-Init([string]$Schema) {
    Assert-CompatSchema $Schema
    Invoke-Psql $BaseDsn @("CREATE SCHEMA IF NOT EXISTS `"$Schema`"") | Out-Null
    Write-Host "[ok] schema $Schema 就绪"
}

function Invoke-Reset([string]$Schema) {
    Assert-CompatSchema $Schema
    Invoke-Psql $BaseDsn @(
        "DROP SCHEMA IF EXISTS `"$Schema`" CASCADE",
        "CREATE SCHEMA `"$Schema`""
    ) | Out-Null
    Write-Host "[ok] schema $Schema 已重置为空"
}

function Invoke-Drop([string]$Schema) {
    Assert-CompatSchema $Schema
    Invoke-Psql $BaseDsn @("DROP SCHEMA IF EXISTS `"$Schema`" CASCADE") | Out-Null
    Write-Host "[ok] schema $Schema 已删除"
}

function Invoke-ApplyDdl([string]$Schema) {
    Assert-CompatSchema $Schema

    Write-Host "[ddl] 从 src/server/bootstrap/*.rs 抽取："
    $statements = Get-BootstrapDdl

    $sqlFile = Join-Path $env:TEMP "contract_ddl_$Schema.sql"
    $body = "SET search_path TO `"$Schema`";`n" + ($statements -join ";`n") + ";`n"
    Set-Content -Path $sqlFile -Value $body -Encoding UTF8 -NoNewline
    Write-Host "[ddl] $($statements.Count) 条语句 -> $sqlFile"

    $dsn = New-SchemaDsn $BaseDsn $Schema
    $out = & $Psql $dsn -v ON_ERROR_STOP=1 -q -f $sqlFile 2>&1
    if ($LASTEXITCODE -ne 0) { throw "灌 DDL 失败（exit $LASTEXITCODE）:`n$($out -join "`n")" }

    $count = (& $Psql $dsn -Atc "select count(*) from pg_tables where schemaname = '$Schema'")
    Write-Host "[ok] $Schema 现有 $count 张表"
}

function Invoke-Backup {
    $pgDump = Resolve-PgTool 'pg_dump'
    if (-not (Test-Path $BackupDir)) { New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $file = Join-Path $BackupDir "db_race-$stamp.sql"
    $out = & $pgDump $BaseDsn -f $file 2>&1
    if ($LASTEXITCODE -ne 0) { throw "pg_dump 失败（exit $LASTEXITCODE）:`n$($out -join "`n")" }
    $size = [math]::Round((Get-Item $file).Length / 1KB, 1)
    Write-Host "[ok] 备份完成：$file ($size KB)"
}

function Invoke-List {
    $sql = @"
select n.nspname,
       (select count(*) from pg_tables t where t.schemaname = n.nspname)
  from pg_namespace n
 where n.nspname like 'compat\_%'
 order by 1
"@
    $out = & $Psql $BaseDsn -At -F ' | ' -c $sql 2>&1
    if ($out) { Write-Host "schema | tables"; $out | ForEach-Object { Write-Host "  $_" } }
    else { Write-Host "（暂无 compat_* schema）" }
}

function Get-RunDir([string]$Schema) {
    return Join-Path $env:TEMP "dsh-compat-run\$Schema"
}

function Invoke-Serve([string]$Schema) {
    Assert-CompatSchema $Schema

    $runDir = Get-RunDir $Schema
    $pidFile = Join-Path $runDir 'server.pid'

    if ($Stop) {
        if (Test-Path $pidFile) {
            $serverPid = (Get-Content $pidFile -Raw).Trim()
            $proc = Get-Process -Id $serverPid -ErrorAction Ignore
            if ($proc) { Stop-Process -Id $serverPid -Force; Write-Host "[ok] 已停止 pid $serverPid" }
            else { Write-Host "[--] pid $serverPid 已不在运行" }
            Remove-Item $pidFile -Force -ErrorAction Ignore
        }
        else { Write-Host "[--] 没有找到 $Schema 的 pid 文件" }
        return
    }

    if ($Build) {
        Write-Host "[cargo] 先构建（注意：本机 sccache 不可用，必须清掉 RUSTC_WRAPPER）"
        $env:CARGO_BUILD_RUSTC_WRAPPER = ''
        & cargo build --manifest-path (Join-Path $DbRoot 'Cargo.toml')
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
    }
    if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，请加 -Build" }

    # 运行目录：config.toml 与 onnx/、static/ 都必须是 CWD 相对路径
    if (-not (Test-Path $runDir)) { New-Item -ItemType Directory -Path $runDir -Force | Out-Null }
    foreach ($link in 'onnx', 'static') {
        $target = Join-Path $DbRoot $link
        $path = Join-Path $runDir $link
        if ((Test-Path $path) -and -not (Get-Item $path).LinkType) {
            throw "$path 是真目录而不是 junction，先手工删掉它"
        }
        if (-not (Test-Path $path)) {
            New-Item -ItemType Junction -Path $path -Target $target | Out-Null
        }
    }

    $cfg = Get-Content $ConfigPath -Raw
    $dsn = New-SchemaDsn $BaseDsn $Schema
    $cfg = [regex]::Replace($cfg, '(?m)^(\s*postgres_url\s*=\s*")[^"]*(")', "`${1}$dsn`${2}")
    $cfg = [regex]::Replace($cfg, '(?m)^(\s*addr\s*=\s*")[^"]*(")', "`${1}127.0.0.1:$Port`${2}")
    Set-Content -Path (Join-Path $runDir 'config.toml') -Value $cfg -Encoding UTF8
    Write-Host "[cfg] $Schema -> $dsn"

    $proc = Start-Process -FilePath $ExePath -WorkingDirectory $runDir -PassThru `
        -RedirectStandardOutput (Join-Path $runDir 'stdout.log') `
        -RedirectStandardError (Join-Path $runDir 'stderr.log')
    Set-Content -Path $pidFile -Value $proc.Id -Encoding ASCII

    $base = "http://127.0.0.1:$Port"
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 500
        if ($proc.HasExited) {
            $err = Get-Content (Join-Path $runDir 'stderr.log') -Raw -ErrorAction Ignore
            throw "服务启动即退出（exit $($proc.ExitCode)）：`n$err"
        }
        try {
            Invoke-WebRequest -Uri "$base/health" -TimeoutSec 2 -UseBasicParsing | Out-Null
            Write-Host "[ok] 服务已就绪 pid=$($proc.Id)"
            Write-Host "     影子契约层：$base/compat/api/..."
            return
        }
        catch { }
    }
    Write-Host "[!!] 等待 /health 超时，进程仍在（pid=$($proc.Id)），日志：$runDir\stdout.log"
}

# --------------------------------------------------------------------------------------
# 分发
# --------------------------------------------------------------------------------------

switch ($PSCmdlet.ParameterSetName) {
    'Init'   { Invoke-Init $Init }
    'Reset'  { Invoke-Reset $Reset }
    'Drop'   { Invoke-Drop $Drop }
    'Apply'  { Invoke-ApplyDdl $Apply }
    'Backup' { Invoke-Backup }
    'List'   { Invoke-List }
    'Serve'  { Invoke-Serve $Serve }
}
