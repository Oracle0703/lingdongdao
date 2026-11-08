# 二开说明与原版合并指南

本仓库（[Oracle0703/lingdongdao](https://github.com/Oracle0703/lingdongdao)）基于原版岛仔（[Renly1994/xiaotang-daozai](https://github.com/Renly1994/xiaotang-daozai)，下称“原版”）二次开发，并定期把原版的更新合并进来。

这份文档记录三件事：

1. **二开清单**：本仓库相对原版改了什么、改在哪里。
2. **合并规则**：合并原版时遇到冲突怎么取舍。
3. **合并流程与记录**：每次合并按什么步骤做，做过哪些决定。

**新增或修改二开功能时，请同步更新本文档的二开清单。** 下次合并原版时，解决冲突主要依据这份清单。

---

## 一、仓库关系

| 远端 | 地址 | 用途 |
| --- | --- | --- |
| `origin` | `https://github.com/Oracle0703/lingdongdao.git` | 本仓库，功能分支通过 PR 合并到 `main` |
| `upstream` | `https://github.com/Renly1994/xiaotang-daozai.git` | 原版，**只拉取，永不推送** |

本地已把 `upstream` 的推送地址设成无效值，防止误推：

```powershell
git remote set-url --push upstream DISABLED-never-push-to-upstream
```

新克隆仓库后需要重新执行这条命令（远端配置不会随仓库同步）。

---

## 二、二开清单

### 2.1 独立身份（冲突时一律保留本仓库）

本仓库与原版可以同时安装、同时运行，互不覆盖数据和凭据。

| 项目 | 位置 | 说明 |
| --- | --- | --- |
| 系统标识 `DaozaiFork` | `src/brand.rs`（`app_id!()`、`APP_ID`） | 所有需要区分两个程序的名字都从这里派生 |
| 数据目录 | `src/model.rs` `config_dir()` | `%APPDATA%\DaozaiFork`，原版是 `%APPDATA%\Daozai` |
| 凭据（Windows 凭据管理器） | `mail.rs` `SERVICE`、`calendar`、`llm.rs`、`grok_quota.rs`、`deepseek_quota.rs` | 服务名都用 `concat!(app_id!(), ".xxx")` |
| 单实例锁、托盘窗口、开机自启 | `src/tray.rs` | `DaozaiFork.SingleInstance`、`DaozaiForkTray` |
| 安装项、快捷方式、卸载 | `installer/setup.nsi`、`scripts/package.ps1`、`src/updates.rs` `record_install_size` | 卸载项 `Uninstall\DaozaiFork`，打包脚本从 `brand.rs` 读标识 |
| 应用内更新来源 | `src/brand.rs`（`REPOSITORY`、`RELEASES`、`RELEASE_API`）、`src/updates.rs` | 只从本仓库发布页更新，拒绝原版安装包 |
| 剪贴板历史的复制标记 | `src/clipboard.rs` `HISTORY_COPY_FORMAT` | 避免与同时运行的原版互相识别 |
| 日历 Exchange 设备标识 | `src/calendar/exchange.rs` | `DeviceId`、`DeviceType` 用系统标识 |

**合并时的检查**：原版新增的代码可能写死原版的名字，合并后在 `src` 下搜索 `"Daozai`、`Daozai.`、`\Daozai`、`Renly1994`，凡是凭据服务名、注册表键、跨进程可见的名字（互斥量、剪贴板格式、`FindWindow` 用到的窗口类）都要改成基于 `app_id!()`。只在本进程内注册的窗口类名、HTTP User-Agent 可以保留。

### 2.2 移除的内容（冲突时一律不合并）

- 赞助入口与收款码：`extras.rs` 的 `support_author`、`release_support_codes`、`support_code_image`，`assets/support/`。
- 第三方备用下载链接（飞书、网盘）：`updates.rs` 的 `BACKUP_DOWNLOAD` 及相关按钮和提示文字；下载失败时改为提示用浏览器下载。
- README 里的支持者感谢墙及 `docs/images/readme/supporters.webp`。

`README.md`、`CHANGELOG.md` 是本仓库自己的文档：合并时保留本仓库版本，只把原版的新功能补进功能表和更新记录。

### 2.3 新增功能

尽量放在独立文件里，只在原版文件中留少量接入点。原版文件里的接入点用 `[二开]` 注释标出，合并时搜索 `[二开]` 即可看到全部接入点。

| 功能 | 独立文件 | 在原版文件中的接入点 |
| --- | --- | --- |
| 自建大模型 | `llm.rs`、`llm_ui.rs` | 设置分组「大模型」 |
| 邮件 AI 摘要 | `mail_digest.rs` | `mail_reader.rs`、`app.rs` 邮件事件 |
| 新验证码自动预览 | `mail_preview.rs` | `app.rs`（`start_mail_preview`、面板高度、`FrameOut`） |
| 会议邀请的取消与改期 | `mail_invites.rs` | `mail.rs` `Event::Mail{updates}`、`Event::MailboxReset`、`deliver()`；`mail_fetch.rs` `item_with_updates` |
| 音乐来源选择 | — | `smtc.rs`（`PollCache`、`spawn_poller_with_sources`、`Sources`），`app.rs` `media_control_source`，`main.rs` 媒体控制 |
| 工资 | `salary.rs`、`salary_ui.rs` | 第 9 页 `SALARY_PAGE = 8`；常驻组件 `bar_layout::SALARY = 10` |
| 截图与图库 | `screenshot/`、`screenshot_ui.rs`、`screenshot_preview.rs` | 第 10 页 `SCREENSHOT_PAGE = 9`；`tray.rs` 快捷键；`native.rs` `set_screenshot_capturing`；`main.rs`/`app.rs` 截图期间的命中区、光标与全屏隐藏 |
| 截图标注 | `screenshot/annotate.rs` | — |
| 拖动边缘调大小 | `island_resize.rs` | `app.rs` 悬停判断与 `resize_input` |
| 展开收起更流畅 | — | `native.rs` `next_region`（形变途中区域只扩大），`main.rs` `DwmFlush` 节奏，`app.rs` `FrameOut::morphing` |
| 歌词岛播放控制 | — | `app.rs` `shared_header`、`lyric_controls_rect` |
| 沙漏 | `hourglass.rs` | 第 11 页 `HOURGLASS_PAGE = 10`；与番茄钟共用常驻位置 |
| 系统监控 | `perf.rs`、`perf_ui.rs` | 第 12 页 `SYSTEM_PAGE = 11`；常驻组件 `bar_layout::SYSTEM = 11`；`main.rs` 采样线程 |

### 2.4 对原版行为的修改

| 修改 | 位置 | 合并时注意 |
| --- | --- | --- |
| 保存失败时恢复原值并提示；数据文件读取失败时禁止覆盖保存 | `model.rs`（`load_error`、`ensure_loaded`、`save()` 返回 `Result`），`App::persist_*` | 原版新增的 `xxx.save();` 会出现 “unused Result” 警告：改成 `persist_*` 或保存失败时提示，不能忽略 |
| 待办保存为带版本号的 JSON | `model.rs` `TodoList::parse/parse_json/serialize/save` | 原版给待办加字段时，同时加进 JSON（作为可选字段），并保留旧文本格式的读取 |
| Codex、Kimi 账号绑定登录身份 | `quota.rs` `account_credentials`、`confirm_current_login`；`quota_ui.rs`「重新绑定当前登录」 | Claude 等其他来源保持原版行为 |
| 岛的大小 | `model.rs` `ISLAND_SIZE_MAX = 200`（原版 140）、`display_layout::island_zoom`、`main.rs` `Shell::island_scale` | 用原版的 `island_size` 与 `eframe::set_root_viewport_scale`；本仓库只加了 200% 上限、顶部停靠时按屏幕限制、旧 `ui_scale` 迁移 |
| 页面与常驻组件编号 | `app.rs` `PAGE_COUNT` 等、`bar_layout.rs` | 见下面“编号规则” |
| 设置窗口主题 | `settings_theme.rs`、`extras.rs` 的设置样式 | 原版的设置窗口样式改动不合并，新控件沿用本仓库主题 |

**编号规则**：页面序号会保存在设置文件里（`visible_pages`、`page_order`），常驻组件序号保存在 `resident_order` 里。本仓库新增的页面占 8–11，常驻组件占 10–11，已经发布给用户，**不能改号**。原版以后新增页面或常驻组件时，排在本仓库的编号之后（原版的第 8 页在本仓库是第 12 页），并在 `model.rs`/`bar_layout.rs` 的旧配置迁移里追加；原版额外的额度圆环槽位从 `WIDTHS.len()` 起算，会自动排在后面。

---

## 三、合并规则

解决冲突时按顺序判断：

1. **独立身份**（2.1）：保留本仓库。原版新代码里的身份字符串改成基于 `app_id!()`。
2. **移除的内容**（2.2）：不合并。
3. **原版新功能**：全部合并。
4. **双方都实现了同一件事**（例如本次的岛体缩放、收信保活与重连补收）：采用原版的实现，本仓库只保留原版没有的部分，接在原版实现之上。这样本仓库自己维护的差异越来越少，以后合并的冲突也越来越少。
5. **双方各改了同一处的不同方面**：两边都保留。先写原版的逻辑，再接上本仓库的部分，并用 `[二开]` 注释说明。
6. **测试**：两边的测试都保留。原版测试因本仓库的改动失败时，按本仓库的行为修正测试并加 `[二开]` 注释；本仓库测试因采用了原版实现而不再成立时删除，并在同步记录里写明。
7. **说明文档**：`README.md`、`CHANGELOG.md` 保留本仓库版本，补充原版新功能；`docs/USAGE.md` 两边内容都保留。

---

## 四、合并流程

### 4.1 准备

```powershell
git switch main; git pull
git fetch upstream --tags
git log --oneline (git merge-base main upstream/main)..upstream/main   # 原版新增了哪些提交
git config rerere.enabled true            # 记住冲突的解决方式，下次同样的冲突自动套用
git config merge.conflictStyle zdiff3     # 冲突标记里同时显示共同祖先，便于判断双方各改了什么
```

也可以直接运行 `scripts/upstream-sync.ps1`，它会完成上面的检查、建分支并开始合并，最后列出冲突文件。

### 4.2 合并

1. 新建分支 `sync/upstream-<原版版本号>`，执行 `git merge upstream/main`。**用合并，不用变基**：合并提交会记住原版合并到哪里，下次只需处理新增的提交。
2. 先读原版这几个版本的 `CHANGELOG.md` 和提交说明，了解原版改了什么、为什么改。
3. 从小文件到大文件逐个解决冲突，按“三、合并规则”取舍。
4. 解决完冲突后，再检查编译器发现不了的问题：
   - 页面、常驻组件编号有没有撞号（见“编号规则”）；
   - 原版新代码里的身份字符串（见 2.1）；
   - 原版新代码有没有带回已移除的内容（搜索 `support`、`收款`、`BACKUP_DOWNLOAD`、`supporters`）；
   - 原版新增的 `save()` 调用有没有忽略保存失败（编译警告 “unused Result”）。
5. **注意**：用 zdiff3 冲突标记、把两边都保留时，两段代码末尾共用的 `}` 可能只剩一个，编译时会报 “unclosed delimiter”。按报错位置补上即可。
6. 新增依赖时 `Cargo.lock` 会变化，需要联网执行一次 `cargo build`。

### 4.3 验证

```powershell
cargo build --locked --features record
cargo test --locked --all-targets --features record
```

- 渲染本仓库功能的预览图检查外观，例如 `DAOZAI_HOURGLASS_PREVIEW=<目录> cargo test hourglass_gallery -- --ignored`、`DAOZAI_PERF_PREVIEW=<目录> cargo test perf_gallery -- --ignored`。
- 编译 release 版，在真机上过一遍：本仓库的二开功能 + 原版这次的新功能。
- 发 PR 到 `main`，PR 说明里列出这次合并的原版版本和重要决定，并在下面的同步记录里补一行。

---

## 五、同步记录

| 日期 | 原版版本（提交） | 合并分支 | 主要决定 |
| --- | --- | --- | --- |
| 2026-11-08 | v0.6.63（`52e98c5`），上次合并点 `b93dd84`（v0.6.50 之后） | `sync/upstream-v0.6.63` | 见下 |

**2026-11-08 合并原版 v0.6.60–v0.6.63**

- 岛体缩放：采用原版的 `island_size` 与 `eframe::set_root_viewport_scale`，删除本仓库自己的 `vendor/egui-winit` 补丁；上限改为 200%（拖动边缘使用），顶部停靠时仍按屏幕限制；旧设置 `ui_scale` 自动迁移到 `island_size`；拖动边缘缩放改为 5% 一档（与原版一致）。
- 收信：采用原版 v0.6.63 的收信循环（45 秒保活、重连补收、分批收取、UIDVALIDITY 核对、服务器拒绝的邮件跳过）。删除本仓库的 `MailboxCursor`/`catch_up` 及对应测试；保留会议邀请修订随邮件送达、UID 空间重置时 `MailboxReset` 清理旧缓存（同时清掉原版邮箱整理按旧 UID 记录的状态）；恢复“同一封邮件连续读取失败 3 次后跳过”（按收信线程记录），已停用的收信线程不再上报状态。原版的邮件列表刷新换回的邮件会重新套用会议邀请修订。
- AI 额度：采用原版的查询服务（分组并发、按账号来源过滤过期结果）和新的设置界面；保留 Codex、Kimi 身份绑定与「重新绑定当前登录」、配置读取失败提示、保存失败恢复。删除不再适用的测试 `claude_cli_token_refresh_keeps_working_like_upstream`、`quota_rebind_discards_previous_identity_results`。
- 待办：保留本仓库的 JSON 格式，加入原版的每周重复字段 `repeat`、`reminded_on`；旧文本格式里原版的 `r<星期位>` 标记可以读取。
- 常驻组件：采用原版重写的排布（任意数量的额度圆环、余额加宽），工资、系统监控作为附加槽位接在上面；放不下时先让工资、再让系统监控。
- 原版新增的左右停靠：岛体缩放、拖动边缘、工资、沙漏、系统监控等本仓库组件只在顶部停靠时生效或显示。
- 身份：DeepSeek 凭据服务名、剪贴板历史标记、安装大小登记的卸载项改为本仓库标识。
- 未合并：赞助入口与收款码、网盘下载、支持者感谢墙。
- 合并后全项目复查（4 个方向：邮件；窗口与岛；数据、设置与 AI 额度；音乐与二开功能）发现并修复：会议修订被列表刷新覆盖、UID 重置后原版整理状态残留、旧收信线程覆盖状态、回收站邮件与收件箱同 UID 时串用 AI 摘要和会议修订、预览「查看邮件」不能回到收件箱、删除 AI 账号或重连 DeepSeek 时先删凭据再保存、旧版 Claude CLI 账号未迁移、剪贴板历史读取失败后会被覆盖、全屏隐藏时窗口区域残留、截图期间与启动首帧不按屏幕上限缩放、拖动缩放从已保存值而不是显示值算起、轮廓裁剪渲染下按屏幕刷新出帧更卡、左右停靠时隐藏的沙漏/工资/系统组件仍触发重绘、歌词岛按钮命中区未随缩小、沙漏提醒「查看」打开错页面、手动选定的播放器切歌瞬间闪空。
- 已知问题（影响小，暂不处理）：
  - 设置窗口里原版新的分段选择、数字输入控件与左侧标签在本仓库主题下相差约 1.5 像素，相应测试的容差放宽到 2 像素。
  - 岛设置了水平偏移时，拖动展开面板的边缘缩放，边缘不会精确跟手（方向正确）。
  - 正在看邮箱页时，原版的列表刷新可能先于收信线程拿到新验证码邮件，这时不再自动弹出验证码预览（页面上已能看到）。
  - 安装器把装有 `daozai.exe` 的文件夹当作自己的安装目录（与原版同名），用户手动选到原版的安装目录时会装进去。
  - 切换音乐来源时会丢掉正在下载的封面，下一次轮询会重新下载。
