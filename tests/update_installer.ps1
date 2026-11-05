# 使用独立目录和最小测试程序验证更新，不启动或替换日常使用的岛仔。
param([switch]$EncryptedCache, [string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$project = Split-Path $PSScriptRoot -Parent
$testRoot = if ($OutputDirectory) { Join-Path $OutputDirectory ('update-test-' + [Guid]::NewGuid().ToString('N')) } else { Join-Path $project ('target\update-test-' + [Guid]::NewGuid().ToString('N')) }
New-Item -ItemType Directory -Path $testRoot | Out-Null
$oldLocal = $env:LOCALAPPDATA
$appId = 'UpdateTestApp' + [Guid]::NewGuid().ToString('N')
try {
    $env:LOCALAPPDATA = if ($EncryptedCache) { Join-Path $oldLocal ('DaozaiUpdateTest-' + [Guid]::NewGuid().ToString('N')) } else { Join-Path $testRoot 'local' }
    New-Item -ItemType Directory -Path $env:LOCALAPPDATA -Force | Out-Null
    if ($EncryptedCache -and -not ((Get-Item -LiteralPath $env:LOCALAPPDATA).Attributes -band [IO.FileAttributes]::Encrypted)) { throw '测试缓存目录必须启用 EFS 加密' }
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    $source = Join-Path $testRoot 'fixture.cs'
    '[assembly:System.Reflection.AssemblyFileVersion("9.8.7")] class Fixture { static void Main() { System.IO.File.WriteAllText(System.IO.Path.Combine(System.AppDomain.CurrentDomain.BaseDirectory,"started.txt"),"ok"); } }' | Set-Content -LiteralPath $source
    $fixture = Join-Path $testRoot 'fixture.exe'
    & $compiler /nologo /target:winexe "/out:$fixture" $source
    if ($LASTEXITCODE -ne 0) { throw '测试程序编译失败' }
    $oldFixture = Join-Path $testRoot 'old-fixture.exe'
    (Get-Content -LiteralPath $source -Raw).Replace('9.8.7', '9.8.6').Replace('"ok"', '"old"') | Set-Content -LiteralPath $source
    & $compiler /nologo /target:winexe "/out:$oldFixture" $source
    if ($LASTEXITCODE -ne 0) { throw '旧版测试程序编译失败' }
    $installer = Join-Path $testRoot 'apply.ps1'
    [IO.File]::WriteAllText($installer, [IO.File]::ReadAllText((Join-Path $project 'installer\update.ps1')), [Text.UTF8Encoding]::new($true))
    $targetDir = Join-Path $testRoot 'installed'
    New-Item -ItemType Directory -Path $targetDir | Out-Null
    $target = Join-Path $targetDir 'daozai.exe'
    $data = Join-Path $targetDir 'settings.json'
    Set-Content -LiteralPath $data -Value 'keep-user-data'
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    foreach ($case in @('success','inherited-shell','missing-old-notices','bad-hash','wrong-version','traversal','missing-license','duplicate-notices','rollback-existing','rollback-new-notices')) {
        Remove-Item -LiteralPath (Join-Path $targetDir 'started.txt') -ErrorAction SilentlyContinue
        Copy-Item -LiteralPath $oldFixture -Destination $target -Force
        $before = (Get-FileHash -LiteralPath $target).Hash
        $originalNotices = @{}
        foreach ($name in @('LICENSE','THIRD_PARTY_NOTICES.txt')) {
            $path = Join-Path $targetDir $name
            if ($case -in @('missing-old-notices','rollback-new-notices')) { Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue }
            else { Set-Content -LiteralPath $path -Value "old-$name" }
            $originalNotices[$name] = if (Test-Path -LiteralPath $path) { (Get-FileHash -LiteralPath $path).Hash } else { $null }
        }
        $work = Join-Path $env:LOCALAPPDATA ($appId + '\Updates\' + $case)
        New-Item -ItemType Directory -Path $work -Force | Out-Null
        $archive = Join-Path $work 'update.zip'
        $zip = [IO.Compression.ZipFile]::Open($archive, [IO.Compression.ZipArchiveMode]::Create)
        $version = if ($case -eq 'wrong-version') {'9.8.8'} else {'9.8.7'}
        $entry = if ($case -eq 'traversal') {'../daozai.exe'} else {"Daozai-v$version-windows-x64/daozai.exe"}
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $fixture, $entry) | Out-Null
        foreach ($name in @('LICENSE','THIRD_PARTY_NOTICES.txt')) {
            if ($case -eq 'missing-license' -and $name -eq 'LICENSE') { continue }
            $copies = if ($case -eq 'duplicate-notices' -and $name -eq 'THIRD_PARTY_NOTICES.txt') { 2 } else { 1 }
            for ($i=0; $i -lt $copies; $i++) {
                $entry = $zip.CreateEntry("Daozai-v$version-windows-x64/$name")
                $writer = [IO.StreamWriter]::new($entry.Open())
                try { $writer.Write("new-$name") } finally { $writer.Dispose() }
            }
        }
        $zip.Dispose()
        Set-Content -LiteralPath (Join-Path $work 'version.txt') -Value $version
        $hash = if ($case -eq 'bad-hash') {'a' * 64} else {(Get-FileHash -LiteralPath $archive).Hash}
        Set-Content -LiteralPath (Join-Path $work 'hash.txt') -Value $hash
        $shell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $lock = $null
        try {
        if ($case -in @('rollback-existing','rollback-new-notices')) {
            # 允许备份读取，但禁止最终 EXE 原子替换，确保已经写入的声明需要回滚。
            $lock = [IO.File]::Open($target, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        }
        if ($case -eq 'inherited-shell') {
            # 模拟应用继承 PowerShell 7 的模块路径，绕过交互式 shell 对子进程环境的自动修正。
            $start = [Diagnostics.ProcessStartInfo]::new($shell)
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $start.EnvironmentVariables['PSModulePath'] = $env:PSModulePath
            $start.Arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$installer`" -WorkPath `"$work`" -TargetExe `"$target`" -ParentProcessId 0 -AppId $appId -Quiet"
            $process = [Diagnostics.Process]::Start($start)
            if (-not $process.WaitForExit(30000)) { throw '继承终端环境的更新测试超时' }
            $code = $process.ExitCode
            $process.Dispose()
        } else {
            & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installer -WorkPath $work -TargetExe $target -ParentProcessId 0 -AppId $appId -Quiet
            $code = $LASTEXITCODE
        }
        } finally { if ($lock) { $lock.Dispose() } }
        if ($case -in @('success','inherited-shell','missing-old-notices')) {
            if ($code -ne 0) { throw "更新安装失败：$case" }
            if ((Get-FileHash -LiteralPath $target).Hash -ne (Get-FileHash -LiteralPath $fixture).Hash) { throw '未替换为新版程序' }
            for ($attempt=0; $attempt -lt 50 -and -not (Test-Path -LiteralPath (Join-Path $targetDir 'started.txt')); $attempt++) { Start-Sleep -Milliseconds 100 }
            if (-not (Test-Path -LiteralPath (Join-Path $targetDir 'started.txt'))) { throw '更新后没有重启' }
            if ((Get-Content -LiteralPath (Join-Path $targetDir 'started.txt') -Raw) -ne 'ok') { throw '启动的不是新版程序' }
            foreach ($name in @('LICENSE','THIRD_PARTY_NOTICES.txt')) {
                if ((Get-Content -LiteralPath (Join-Path $targetDir $name) -Raw) -ne "new-$name") { throw "未更新声明：$name" }
            }
        } else {
            if ($code -eq 0 -or -not (Test-Path -LiteralPath (Join-Path $work 'error.txt'))) { throw "未拒绝无效更新：$case" }
            if ((Get-FileHash -LiteralPath $target).Hash -ne $before) { throw '无效更新修改了原程序' }
            foreach ($name in @('LICENSE','THIRD_PARTY_NOTICES.txt')) {
                $path = Join-Path $targetDir $name
                $actual = if (Test-Path -LiteralPath $path) { (Get-FileHash -LiteralPath $path).Hash } else { $null }
                if ($actual -ne $originalNotices[$name]) { throw "失败后声明未恢复：$name" }
            }
            if ($case -like 'rollback-*') {
                if (-not (Test-Path -LiteralPath (Join-Path $work 'ready'))) { throw '回滚测试没有到达文件替换阶段' }
                if ((Get-Content -LiteralPath (Join-Path $work 'error.txt') -Raw) -match '恢复未完成') { throw '回滚未完成' }
            }
        }
        if (Get-ChildItem -LiteralPath $targetDir -Filter '.daozai-update-*') { throw '残留更新暂存文件' }
        if ((Get-Content -LiteralPath $data -Raw).Trim() -ne 'keep-user-data') { throw '用户数据被修改' }
        Write-Output "通过：$case"
    }
} finally {
    $env:LOCALAPPDATA = $oldLocal
}
