#requires -Version 7
<#
.SYNOPSIS
    交叉编译 trace-hub（aarch64-linux）并部署到 G10 设备。

.DESCRIPTION
    Windows 开发机 → 用预置交叉编译镜像在 Docker 容器内构建 aarch64-unknown-linux-gnu
    的 release 二进制（带 prod feature，日志落文件、stdout 保持干净）→ scp 到 G10，
    经 `trace-hub install` 重装 user systemd unit（把 -Bind 写进 Environment=TRACE_HUB_BIND）。

    镜像 huangjiemin/rust_aarch64-gcc_openssl 已预置：aarch64 gcc/ar/g++、cmake、clang，
    以及 CARGO_TARGET_*_LINKER / CC_/CXX_ 环境变量。custom-utils 已改走 crates.io 0.16，
    容器内直接从 registry 拉，无需挂载兄弟仓 path。

.PARAMETER G10Host
    G10 的 ssh 目标，默认 fengqi@192.168.0.68。

.PARAMETER DestDir
    G10 上的安装目录，默认 ~/.local/bin（与 custom-utils updater 自更新目标一致）。

.PARAMETER Service
    部署后要重启的 systemd 用户服务名，默认 trace-hub。

.PARAMETER Bind
    trace-hub 的监听地址，默认 0.0.0.0:9120。部署时通过 `trace-hub install -e TRACE_HUB_BIND=<Bind>`
    写进 systemd unit 的 `Environment=TRACE_HUB_BIND=<Bind>`（重装 unit 使新端口生效）。
    serve 时 TRACE_HUB_BIND 优先于 config.toml。G10 部署面板把该服务 registry 主端口拼成
    `0.0.0.0:<port>` 传进来。

.PARAMETER Workspace
    trace-hub 的 workspace 根目录（远端路径），默认 ~/.config/trace-hub（与 install 默认一致）。
    install 时显式传给 `--workspace`。

.PARAMETER Env
    额外注入 systemd unit 的环境变量，`KEY=VAL` 数组（逗号分隔）。install 时逐条转发为
    `-e KEY=VAL`（custom-utils 0.16 写进 unit 的 `Environment=`），键冲突时覆盖内置默认。
    例：`-Env TRACE_HUB_ENDPOINT=...`。

.PARAMETER SkipBuild
    跳过交叉编译，直接复制已有产物（调试部署用）。

.PARAMETER SkipRestart
    跳过部署后重启（仅换二进制，下次服务自然重启时生效）。

.EXAMPLE
    pwsh ./deploy-g10.ps1
    pwsh ./deploy-g10.ps1 -SkipBuild
    pwsh ./deploy-g10.ps1 -Bind 0.0.0.0:9121
#>
param(
    [string]$G10Host = "fengqi@192.168.0.68",
    [string]$DestDir = "~/.local/bin",
    [string]$Service = "trace-hub",
    [string]$Bind = "0.0.0.0:9120",
    [string]$Workspace = "~/.config/trace-hub",
    [string[]]$Env = @(),
    [switch]$SkipBuild,
    [switch]$SkipRestart
)

$ErrorActionPreference = "Stop"
$RepoRoot = $PSScriptRoot
$Target = "aarch64-unknown-linux-gnu"
$Image = "huangjiemin/rust_aarch64-gcc_openssl:1.94.0_9.4.0_1.1.0l_llvm12.0.1"

# (crate package 名, 产物二进制名) —— 新增工具时在此追加一行即可。
$Bins = @(
    @{ Crate = "trace-hub"; Bin = "trace-hub" }
)

# 产物输出目录（host 可见，从容器内的 CARGO_TARGET_DIR 拷出来）。
# 改用 dist/g10 而非 target/ 是为了把 CARGO_TARGET_DIR 放进命名卷（Linux ext4），
# 避免 Windows NTFS 经 Docker Desktop 的 mtime/权限抖动让 cargo 指纹失效每次全量重编。
$OutDir = Join-Path $RepoRoot "dist/g10"

if (-not $SkipBuild) {
    Write-Host "==> 交叉编译 $Target（Docker: $Image）" -ForegroundColor Cyan
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "未找到 docker，请先安装/启动 Docker Desktop。"
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

    # 逐 crate 带 prod feature 构建（virtual workspace 必须 -p）。
    # 构建后把所有 bin 从容器内 CARGO_TARGET_DIR（命名卷）拷到 host 可见的 /work/dist/g10/。
    $cargoBuild = ($Bins | ForEach-Object {
        "cargo build --release --target $Target -p $($_.Crate) --features prod"
    }) -join " && "
    $copyCmd = ($Bins | ForEach-Object {
        "cp /cargo-target/$Target/release/$($_.Bin) /work/dist/g10/"
    }) -join " && "
    $buildCmd = "$cargoBuild && mkdir -p /work/dist/g10 && $copyCmd"

    # 命名卷缓存：
    #   - cargo registry（依赖源/索引）
    #   - cargo target（编译产物指纹；放命名卷 = Linux ext4，避免 NTFS 经 Docker Desktop 时
    #     mtime 抖动导致 cargo 每次都重编）
    # custom-utils 走 crates.io，无需挂载兄弟仓 path。
    # AR_ 显式补上（镜像只预置了 CC_/CXX_/LINKER）。
    docker run --rm `
        -v "${RepoRoot}:/work" `
        -v "trace-hub-cargo-registry:/root/.cargo/registry" `
        -v "trace-hub-cargo-target:/cargo-target" `
        -w /work `
        -e CARGO_TARGET_DIR=/cargo-target `
        -e AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar `
        $Image bash -lc $buildCmd
    if ($LASTEXITCODE -ne 0) { throw "交叉编译失败（exit $LASTEXITCODE）" }
}

# 校验产物存在（统一在 dist/g10 下；-SkipBuild 时也读这里）。
$ReleaseDir = $OutDir
foreach ($b in $Bins) {
    $p = Join-Path $ReleaseDir $b.Bin
    if (-not (Test-Path $p)) { throw "产物缺失：$p（先去掉 -SkipBuild 完整构建）" }
}

Write-Host "==> 部署到 ${G10Host}:${DestDir}" -ForegroundColor Cyan
# 确保远端目录存在。
ssh $G10Host "mkdir -p $DestDir"
if ($LASTEXITCODE -ne 0) { throw "无法在 G10 创建目录 $DestDir（检查 ssh 连通性）" }

foreach ($b in $Bins) {
    $local = Join-Path $ReleaseDir $b.Bin
    $dest = "$DestDir/$($b.Bin)"
    Write-Host "    scp $($b.Bin)"
    # 先传到 .new 临时名，再 mv 覆盖：rename 即使旧二进制正在运行也能替换
    # （直接 scp 覆盖运行中的二进制会 ETXTBSY / dest open Failure）。
    scp $local "${G10Host}:${dest}.new"
    if ($LASTEXITCODE -ne 0) { throw "scp $($b.Bin) 失败" }
    ssh $G10Host "chmod +x ${dest}.new && mv -f ${dest}.new ${dest}"
    if ($LASTEXITCODE -ne 0) { throw "替换 $($b.Bin) 失败（mv）" }
}

# 打印版本确认。
Write-Host "==> 远端版本确认" -ForegroundColor Cyan
foreach ($b in $Bins) {
    ssh $G10Host "$DestDir/$($b.Bin) --version"
}

# 重装 trace-hub unit：把 -Bind 写进 unit 的 Environment=TRACE_HUB_BIND=<Bind>，
# 使新端口生效（install 幂等：重写 unit + daemon-reload）。
if ($Service -eq "trace-hub") {
    Write-Host "==> 重装 trace-hub unit（TRACE_HUB_BIND=$Bind, workspace=$Workspace）" -ForegroundColor Cyan
    # 把每条 KEY=VAL 拼成 `-e 'KEY=VAL'`（单引号防远端 shell 二次解析），追加进 install 命令。
    # 注：面板把多条 env 拼成 "K1=V1,K2=V2,K3=V3" 作单参传入（`[string[]]` 从 `pwsh -File`
    # 单 argv 不会自动拆逗号），故先按逗号展开再逐条转 `-e`。
    $envArgs = ($Env | Where-Object { $_ -and $_.Trim() -ne "" } | ForEach-Object {
        $_.Split(",") | Where-Object { $_.Trim() -ne "" } | ForEach-Object { "-e '$($_.Trim())'" }
    }) -join " "
    if ($envArgs) {
        Write-Host "    注入环境变量：$($Env -join ', ')" -ForegroundColor DarkGray
    }
    $installCmd = 'export XDG_RUNTIME_DIR=/run/user/$(id -u); ' `
        + "$DestDir/trace-hub install --workspace $Workspace -e TRACE_HUB_BIND=$Bind $envArgs"
    ssh $G10Host $installCmd
    if ($LASTEXITCODE -ne 0) { throw "trace-hub install 失败（重装 unit）" }
}

# 重启用户服务。XDG_RUNTIME_DIR 显式补上：非交互 ssh 默认不带，systemctl --user 会找不到 user bus。
if (-not $SkipRestart) {
    Write-Host "==> 重启 $Service" -ForegroundColor Cyan
    $restartCmd = 'export XDG_RUNTIME_DIR=/run/user/$(id -u); ' `
        + "systemctl --user restart $Service && " `
        + "sleep 2 && " `
        + "systemctl --user is-active $Service && " `
        + "systemctl --user status $Service --no-pager -n 5"
    ssh $G10Host $restartCmd
    if ($LASTEXITCODE -ne 0) { throw "重启 $Service 失败（检查服务名 / 是否已 systemctl --user enable）" }
} else {
    Write-Host "==> 跳过重启（-SkipRestart）" -ForegroundColor DarkGray
}

Write-Host "==> 完成。健康检查：http://<g10>:$($Bind.Split(':')[-1])/health" -ForegroundColor Green
