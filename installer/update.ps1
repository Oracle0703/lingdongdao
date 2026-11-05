param([Parameter(Mandatory=$true)][string]$WorkPath, [Parameter(Mandatory=$true)][string]$TargetExe, [int]$ParentProcessId, [Parameter(Mandatory=$true)][ValidatePattern('^[A-Za-z0-9._-]+$')][string]$AppId, [switch]$Quiet)
$ErrorActionPreference = 'Stop'
# 更新只使用当前 PowerShell 自带模块，避免继承其他终端版本的模块路径。
$env:PSModulePath = [IO.Path]::Combine($PSHOME, 'Modules')
$cacheRoot = [IO.Path]::GetFullPath((Join-Path (Join-Path $env:LOCALAPPDATA $AppId) 'Updates')).TrimEnd('\')
$workRoot = [IO.Path]::GetFullPath($WorkPath).TrimEnd('\')
$target = [IO.Path]::GetFullPath($TargetExe)
$targetDir = Split-Path $target -Parent
$transaction = [Guid]::NewGuid().ToString('N')
# 最后替换程序；在全部文件就绪前不启动新版。
$files = @('LICENSE', 'THIRD_PARTY_NOTICES.txt', 'daozai.exe') | ForEach-Object {
    [pscustomobject]@{
        Name = $_
        Target = $(if ($_ -eq 'daozai.exe') { $target } else { Join-Path $targetDir $_ })
        Staged = Join-Path $targetDir ('.daozai-update-' + $transaction + '-' + $_)
        Backup = Join-Path $workRoot ('previous-' + $_)
        Existed = $false
        Applied = $false
    }
}
$exited = $false
function Assert-PlainPath([string]$PathToCheck) {
    $ancestor = [IO.Path]::GetFullPath($PathToCheck)
    while ($ancestor) {
        if ((Test-Path -LiteralPath $ancestor) -and ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw '更新目录不能包含目录链接。' }
        $ancestor = Split-Path $ancestor -Parent
    }
}
try {
    if (-not $workRoot.StartsWith($cacheRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw '更新缓存目录无效。' }
    Assert-PlainPath $workRoot
    Assert-PlainPath $target
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) { throw '找不到当前程序。' }
    $version = (Get-Content -LiteralPath (Join-Path $workRoot 'version.txt') -Raw).Trim()
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw '更新版本号无效。' }
    $archive = Join-Path $workRoot 'update.zip'
    $expectedHash = (Get-Content -LiteralPath (Join-Path $workRoot 'hash.txt') -Raw).Trim()
    if ($expectedHash -notmatch '^[a-fA-F0-9]{64}$' -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expectedHash) { throw '更新包校验失败。' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        foreach ($file in $files) {
            Assert-PlainPath $file.Target
            if ((Test-Path -LiteralPath $file.Target) -and -not (Test-Path -LiteralPath $file.Target -PathType Leaf)) { throw "更新目标不是文件：$($file.Name)" }
            $name = 'Daozai-v' + $version + '-windows-x64/' + $file.Name
            $entries = @($zip.Entries | Where-Object {$_.FullName.Replace('\','/') -ceq $name})
            $limit = if ($file.Name -eq 'daozai.exe') { 200MB } else { 32MB }
            if ($entries.Count -ne 1 -or $entries[0].Length -eq 0 -or $entries[0].Length -gt $limit) { throw "更新包中缺少唯一且有效的文件：$($file.Name)" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], $file.Staged)
        }
    } finally { $zip.Dispose() }
    $actual = [Diagnostics.FileVersionInfo]::GetVersionInfo($files[-1].Staged).FileVersion
    if ($actual -ne $version -and $actual -ne "$version.0") { throw '安装包版本不匹配。' }
    Set-Content -LiteralPath (Join-Path $workRoot 'ready') -Value 'ok'
    if ($ParentProcessId -gt 0) {
        $parent = Get-Process -Id $ParentProcessId -ErrorAction SilentlyContinue
        if ($parent -and -not $parent.WaitForExit(60000)) { throw '岛仔尚未退出，请稍后重试。' }
    }
    $exited = $true
    # 先备份所有旧文件，再开始替换；复制允许缓存与安装目录使用不同 EFS 设置。
    foreach ($file in $files) {
        $file.Existed = Test-Path -LiteralPath $file.Target -PathType Leaf
        if ($file.Existed) { [IO.File]::Copy($file.Target, $file.Backup, $true) }
    }
    foreach ($file in $files) {
        if ($file.Existed) { [IO.File]::Replace($file.Staged, $file.Target, [NullString]::Value) }
        else { [IO.File]::Move($file.Staged, $file.Target) }
        $file.Applied = $true
    }
    Start-Process -FilePath $target -WorkingDirectory $targetDir -WindowStyle Hidden
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $AppId
    $registered = Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue
    if ($registered -and [string]::Equals(([string]$registered.InstallLocation).TrimEnd('\'), $targetDir, [StringComparison]::OrdinalIgnoreCase)) {
        Set-ItemProperty -LiteralPath $key -Name DisplayVersion -Value $version -ErrorAction SilentlyContinue
    }
    # 只清理已经确认位于专用缓存内的本次更新目录。
    if ($workRoot.StartsWith($cacheRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        try { Assert-PlainPath $workRoot; Remove-Item -LiteralPath $workRoot -Recurse -Force } catch { }
    }
} catch {
    $message = $_.Exception.Message
    $rollbackErrors = @()
    for ($i = $files.Count - 1; $i -ge 0; $i--) {
        $file = $files[$i]
        try {
            if ($file.Applied) {
                if ($file.Existed) { [IO.File]::Copy($file.Backup, $file.Target, $true) }
                else { Remove-Item -LiteralPath $file.Target -Force }
            }
            if (Test-Path -LiteralPath $file.Staged) { Remove-Item -LiteralPath $file.Staged -Force }
        } catch { $rollbackErrors += "$($file.Name)：$($_.Exception.Message)" }
    }
    if ($rollbackErrors.Count) { $message += "`r`n恢复未完成，备份保留在 ${workRoot}：" + ($rollbackErrors -join '；') }
    if ($workRoot.StartsWith($cacheRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        try {
            Assert-PlainPath $workRoot
            [IO.File]::WriteAllText((Join-Path $workRoot 'error.txt'), $message)
        } catch { }
    }
    if ($exited -and $ParentProcessId -gt 0 -and -not $rollbackErrors.Count) {
        try { Start-Process -FilePath $target -WorkingDirectory $targetDir -WindowStyle Hidden } catch { }
    }
    if ($exited -and -not $Quiet) {
        Add-Type -AssemblyName System.Windows.Forms
        $recovery = if ($rollbackErrors.Count) { '请使用保留的备份恢复，或通过浏览器下载安装。' } else { '原版本已保留，可稍后重试或通过浏览器下载。' }
        [System.Windows.Forms.MessageBox]::Show("更新未完成：$message`r`n$recovery", '岛仔更新') | Out-Null
    }
    exit 1
}
