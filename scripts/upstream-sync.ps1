# 开始一次原版合并：检查工作区、拉取原版、新建 sync 分支并开始合并，列出冲突文件和需要人工检查的地方。
# 不会提交，也不会推送。之后按 docs/FORK.md 的“合并规则”逐个解决冲突。
param([string]$Branch)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

if (git status --porcelain) { throw '工作区有未提交的改动，请先提交或暂存。' }

# 原版只拉取不推送；冲突解决方式记下来，下次同样的冲突自动套用；冲突标记显示共同祖先。
if (-not ((git remote) -contains 'upstream')) { git remote add upstream https://github.com/Renly1994/xiaotang-daozai.git }
git remote set-url --push upstream DISABLED-never-push-to-upstream
git config rerere.enabled true
git config merge.conflictStyle zdiff3

git fetch origin
git fetch upstream --tags
$base = git merge-base origin/main upstream/main
$count = [int](git rev-list --count "$base..upstream/main")
if ($count -eq 0) { Write-Output '原版没有新的提交。'; return }
Write-Output "原版新增 $count 个提交："
git log --oneline "$base..upstream/main"

$version = [regex]::Match((git show upstream/main:Cargo.toml) -join "`n", '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $Branch) { $Branch = "sync/upstream-v$version" }
git switch -c $Branch origin/main
git merge --no-ff --no-commit upstream/main

Write-Output ''
$conflicts = @(git diff --name-only --diff-filter=U)
if ($conflicts.Count -gt 0) {
    Write-Output "有冲突的文件（$($conflicts.Count) 个，括号里是冲突块数）："
    foreach ($file in $conflicts) {
        $blocks = @(Select-String -LiteralPath $file -Pattern '^<<<<<<<' -ErrorAction SilentlyContinue).Count
        Write-Output "  $file ($blocks)"
    }
} else {
    Write-Output '没有文本冲突，仍需按下面的清单检查。'
}

Write-Output ''
Write-Output '原版代码中的身份字符串（凭据、注册表、跨进程名字需改为 app_id!()）：'
git grep -n -E '"Daozai[^F]|Daozai\\\\|Renly1994' -- 'src/*.rs' ':!src/*tests.rs' ':!src/updates.rs'
Write-Output ''
Write-Output '可能带回的已移除内容（赞助、收款码、网盘下载、感谢墙）：'
git grep -n -i -E 'support_author|收款|BACKUP_DOWNLOAD|supporters' -- src README.md
Write-Output ''
Write-Output "下一步：按 docs/FORK.md 解决冲突并检查编号、保存失败处理；完成后运行 cargo test --locked --all-targets --features record。"
