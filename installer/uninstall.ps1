﻿$ErrorActionPreference='Stop'
# 与 src/brand.rs 的 APP_ID 保持一致。
$appId='DaozaiFork'
$shortcutName="岛仔（$appId）"
$installDir=[System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA ('Programs\'+$appId)))
if([System.IO.Path]::GetFullPath($PSScriptRoot) -ne $installDir){throw '卸载脚本只能在岛仔安装目录中运行。'}
$exePath=Join-Path $installDir 'daozai.exe'
Get-Process daozai -ErrorAction SilentlyContinue | Where-Object {$_.Path -eq $exePath} | ForEach-Object {
    $null=$_.CloseMainWindow();if(-not $_.WaitForExit(3000)){Stop-Process -Id $_.Id}
}
$shell=New-Object -ComObject WScript.Shell
$links=@((Join-Path $shell.SpecialFolders.Item('Desktop') "$shortcutName.lnk"),(Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$shortcutName.lnk"),(Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\${shortcutName}设置.lnk"))
foreach($path in $links){if((Test-Path -LiteralPath $path) -and $shell.CreateShortcut($path).TargetPath -eq $exePath){Remove-Item -LiteralPath $path}}
$runKey='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if((Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue).$appId -eq ('"'+$exePath+'"')){Remove-ItemProperty -Path $runKey -Name $appId -ErrorAction SilentlyContinue}
$key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\'+$appId
if((Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).InstallLocation -eq $installDir){Remove-Item -LiteralPath $key -ErrorAction SilentlyContinue}
foreach($name in @('daozai.exe','daozai-previous.exe','daozai-v0.2-backup.exe','THIRD_PARTY_NOTICES.txt','LICENSE','uninstall.ps1')){
    $path=Join-Path $installDir $name;if(Test-Path -LiteralPath $path){Remove-Item -LiteralPath $path}
}
if(-not(Get-ChildItem -LiteralPath $installDir -Force)){Remove-Item -LiteralPath $installDir}
Write-Output '岛仔已卸载。待办、饮水记录和设置仍保存在本机用户数据目录中。'
