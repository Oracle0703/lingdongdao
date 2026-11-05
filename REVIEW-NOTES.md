# 改动说明（供代码审核）

> 后续进展：本文件保留 Claude 上一轮的实现说明；2026-11-05 已继续完成自有更新源、安装隔离和八项可靠性修复，当前状态见 [独立仓库接入与可靠性修复](docs/FORK-READINESS-20261105.md)。下文“刻意未改”和“未提交”描述的是记录当时的范围及工作状态。

基线提交：`b93dd84`（main）。全部改动未提交，查看方式：

```bash
git diff                                   # 已有文件的修改
git status --short                         # 新增文件：src/brand.rs、src/llm.rs、src/llm_ui.rs
```

本次包含三部分：

1. 把与原版岛仔冲突的系统标识改为可单点修改的 `APP_ID`；
2. 新增自建大模型（OpenAI 兼容接口）的基础接入与设置页；
3. 修复两个原有的不稳定测试，以及一个在 Windows PowerShell 5.1 下无法运行的测试脚本。

不在本次范围、刻意未改的内容见第 4 节。

---

## 1. 系统标识改为 `APP_ID`

### 依据

本仓库是 `Renly1994/xiaotang-daozai`（MIT）的分支，以下标识与原版完全相同。两个版本在同一台机器上会互相覆盖数据，或者因单实例锁导致只能启动一个：

| 标识 | 原值 | 冲突后果 |
|---|---|---|
| 数据目录 | `%APPDATA%\Daozai` | 设置、待办、账号元数据互相覆盖 |
| 凭据管理器服务名 | `Daozai.Mail` / `Daozai.Calendar` / `Daozai.GrokBot` | 邮箱、日历授权码互相覆盖 |
| 单实例互斥量 | `Local\Daozai.SingleInstance` | 两个版本只能运行一个 |
| 托盘窗口类 | `DaozaiTray` | 二次启动时会激活另一个版本的窗口 |
| 开机自启值、卸载注册表键 | `HKCU\...\Run\Daozai`、`HKCU\...\Uninstall\Daozai` | 安装或卸载一个版本会改动另一个 |
| 更新缓存 | `%LOCALAPPDATA%\Daozai\Updates` | 更新脚本的路径校验会混用 |
| Exchange ActiveSync 设备标识 | `DaozaiCalendar01` / `Daozai` | 服务器按设备记录同步状态，两个版本会互相干扰 |

产品方要求：界面名称暂时保留"岛仔"，只改系统标识，正式名称以后再定。所以标识集中到一处，取临时值 `DaozaiFork`。

### 改动

- **新增 `src/brand.rs`**：`app_id!()` 宏和 `APP_ID` 常量。用宏的原因是 `concat!` 只接受字面量，这样才能在编译期拼出 `"DaozaiFork.Mail"` 这类常量。
- `src/lib.rs`：注册 `brand`、`llm`、`llm_ui` 模块。
- `src/model.rs:25`：`config_dir()` 改用 `APP_ID`；`read_kv`/`write_kv` 改为 `pub(crate)`，供 `llm.rs` 复用同一套 kv 文件读写（写入仍走原有的 `atomic_write`）；同步更新文件头注释。
- `src/mail.rs:15`、`src/calendar.rs:16`、`src/grok_quota.rs:85`：凭据服务名改为 `concat!(app_id!(), ".Xxx")`。
- `src/calendar/exchange.rs:90`：ActiveSync 的 `DeviceId`/`DeviceType` 改用 `APP_ID`。
- `src/tray.rs:51-54`：`RUN_VALUE`、`MUTEX_NAME` 改值，新增 `TRAY_CLASS` 常量，替换两处原本写死的 `w!("DaozaiTray")`（`:382`、`:651`）。`w!()` 只接受字面量，没法引用 `APP_ID`，因此新增测试 `tray_identifiers_follow_app_id`（`:577`）来校验三者与 `APP_ID` 一致。
- `src/updates.rs:307`：更新缓存目录改为 `%LOCALAPPDATA%\<APP_ID>\Updates`。
- `src/updates.rs:351`：启动更新脚本时传入 `-AppId`。
- `installer/update.ps1`：新增必填参数 `-AppId`，用 `ValidatePattern('^[A-Za-z0-9._-]+$')` 防止路径注入；缓存根目录和卸载注册表键都由它拼出。原有的"工作目录必须位于缓存根目录内"校验逻辑没有改。该脚本通过 `include_str!` 编译进程序，所以旧版程序始终使用自己内置的旧脚本，不存在新旧参数不匹配的问题。
- `installer/install.ps1`、`installer/uninstall.ps1`：顶部新增 `$appId='DaozaiFork'`，安装目录、注册表键、Run 值都由它拼出。这两个脚本单独运行、读不到 Rust 源码，只能手动保持一致，已加注释说明。
- `installer/setup.nsi:10`：`APP_ID` 默认值改为 `DaozaiFork`。
- `scripts/package.ps1`：用正则从 `src/brand.rs` 读取 `APP_ID`，通过 `/DAPP_ID=` 传给 NSIS，读取失败就中止打包。
- `tests/update_installer.ps1`：调用更新脚本时传 `-AppId`，测试用的缓存目录跟着改。
- `README.md`、`docs/PRIVACY.md`：数据目录和更新缓存路径同步更新。

### 审核关注点

- 有没有遗漏的 `Daozai` 系统标识。核对命令：`git grep -n "Daozai" -- src installer scripts`。剩下的应只有界面文字、安装包文件名 `Daozai-v*.zip`、HTTP `User-Agent` 和环境变量 `DAOZAI_DATA_DIR`，这些不会造成冲突。
- `package.ps1` 的正则要能匹配 `brand.rs` 当前的写法。已在本机用 PowerShell 5.1 验证，输出 `APP_ID=DaozaiFork`。

---

## 2. 自建大模型基础接入

### 依据

团队在内网自建了 gpt-oss-20b / 120b，对外提供 OpenAI 兼容接口，需要接入程序，供后续的邮件摘要、日程识别等功能使用。本次只做基础设施：配置、密钥存储、通用调用、设置页和连接测试，不改任何现有功能的行为。

需求约束与对应设计：

| 约束 | 设计 |
|---|---|
| 内网地址可能是 http，或由企业 CA 签发的 https 证书 | 项目原有的 ureq 只开了 `tls` 特性（rustls + webpki-roots 内置根证书），**不读 Windows 证书库**，企业 CA 会校验失败。现在 Windows 平台额外开启 ureq 的 `native-tls` 特性，**只有**大模型客户端通过 `tls_connector` 改用 schannel（Windows 证书库）。已确认 ureq 2.12.1 源码（`lib.rs:405-409`）：同时启用两个特性时默认后端仍是 rustls，**其余请求行为不变** |
| API Key 不能明文落盘 | 只存入 Windows 凭据管理器（服务名 `<APP_ID>.LLM`，账户名 `default`）；`llm.txt` 只保存地址、模型名、推理强度和超时 |
| 不能把 API Key 发到内网以外 | `redirects(0)` 不跟随跳转，收到 3xx 直接报错；`try_proxy_from_env(false)` 不经过代理 |
| gpt-oss 会输出推理过程 | 只读取 `choices[0].message.content`，忽略 `reasoning_content`/`reasoning` 字段；额外去掉混在正文里的 `<think>…</think>`；正文为空且 `finish_reason=length` 时提示"推理用完了生成上限" |
| 网络请求不能阻塞界面 | `Client` 的方法都是阻塞调用，设置页在后台线程中执行，结果经 `mpsc` 回传，再调用 `request_repaint` |
| 服务异常时不能拖垮程序 | 响应体上限 4 MB，错误详情截断到 200 字符；连接超时 5 秒，读取超时可配置（10–600 秒，默认 60）；不使用 `unwrap`，避免在 release 的 `panic = "abort"` 下崩溃 |

### 改动

- **`Cargo.toml`**：在 `[target.'cfg(windows)'.dependencies]` 中给 ureq 加上 `native-tls` 特性（Cargo 会和通用依赖的特性合并）。限定 Windows 是为了不让 Linux 下的录制器构建引入 OpenSSL。`native-tls` 0.2.18 原本就因 IMAP 在依赖树中，`Cargo.lock` 只多了一行（ureq 的依赖列表）。
- **新增 `src/llm.rs`**：
  - `Config`（`:58`），`load`/`save` 读写 `llm.txt`（`:82`、`:100`），读取时对超时做范围限制，非法的推理强度回落为默认值 `low`。
  - `endpoint()`（`:119`）把用户填写的地址规范化：
    - 没写协议的补上 `http://`；
    - 去掉末尾的 `/chat/completions`、`/models`，路径为空时默认 `/v1`，丢弃 query 和 fragment；
    - 拒绝含账号密码的地址和非 http(s) 协议。
  - `Client::new`/`from_saved`（`:199`、`:210`）：`from_saved` 在总开关关闭时直接返回错误，这是后续功能的统一入口。
  - `chat()`（`:222`）：请求体为 `model`、`messages`、`max_tokens`、`stream:false`，可选 `reasoning_effort`（推理强度选"不指定"时不发送）、`temperature`、`response_format: json_object`。
  - `models()`（`:248`）：调用 `GET /models`，结果排序去重。
  - `test()`（`:264`）：先查模型列表（服务不支持则跳过），确认模型存在后，再发一次固定的 "ping"，`max_tokens=512`，给推理模型留出余量。
  - `describe()`（`:331`）：把 HTTP 状态码、DNS 失败、连接失败、超时、TLS 失败映射成中文提示。TLS 失败按 ureq 的 `native_tls connect failed` 消息识别，并附上 schannel 的原始错误。
  - `secrets`（`:431`）：凭据读写；非 Windows 平台是空实现。
- **新增 `src/llm_ui.rs`**：设置页的 `State` 和 `App::llm_settings`。
  - 首次打开时才加载配置、读取凭据，不在程序启动时读取。
  - 表单需要点"保存"才生效，有未保存修改时显示提示。这一点与其他设置页的"自动保存"不同：连接配置不希望改到一半就生效，也避免每次按键都写盘。
  - 启用总开关时，保存前会校验地址和模型名；API Key 输入框留空表示保留已存的密钥。
  - "测试连接"优先使用输入框里新填的 Key，没填才用已保存的。
- `src/app.rs:422`、`:518`：`App` 新增字段 `llm` 并初始化。
- `src/extras.rs`：
  - 设置分组 `("AI 额度",11,11)` 改为 `("AI 额度与模型",11,12)`，新增第 12 页"大模型"；
  - `titles` 数组加一项，`min(11)` 改为 `min(12)`，`match` 增加 `12=>self.llm_settings(ui)`；
  - 同步更新 `settings_groups_keep_every_page_reachable` 测试的期望值。
- `src/ui_tests.rs`：
  - 新增 `llm_settings_show_every_field_inside_the_small_settings_window`：在最小窗口尺寸 600×450 下，所有控件都必须完整显示。这个测试最初失败过，因为按钮被挤出了可视区；之后把表单改得更紧凑，测试才通过。
  - 设置页截图导出测试的页码范围从 `0..12` 改为 `0..13`。
- `docs/PRIVACY.md`：说明大模型配置的存放位置、何时发送请求、发送什么内容。

### 审核关注点

- `endpoint()` 的规范化规则，见测试 `endpoint_accepts_common_base_url_forms` 中列出的用例。
- `describe()` 中，连接阶段的超时判断依赖 ureq `Transport::source()` 能向下转换成 `io::Error`。
- `llm_ui.rs` 在界面线程中做的事：读取凭据管理器、创建 `native_tls::TlsConnector`。两者都是本机调用，耗时可以忽略，网络请求都在后台线程。
- `llm.rs` 的 `config_round_trips_and_clamps` 会修改进程级环境变量 `DAOZAI_DATA_DIR`。这和仓库已有的测试一样，依赖单线程运行（`--test-threads=1`），没有使用 `ui_tests` 里的 `ENV_LOCK`。

---

## 3. 测试修复（只改了测试代码）

- `src/calendar.rs:963`（`discovery_root_errors_follow_well_known_principal_and_home`）、`src/updates.rs:401`（`update_requests_use_configured_proxy`）：
  - 问题：模拟服务器的监听 socket 设成了非阻塞，Winsock 下 `accept` 得到的连接会继承非阻塞模式，客户端数据还没到时 `read()` 返回 `WouldBlock`（10035），`unwrap` 随即 panic。
  - 实际出现过：一次完整测试中因机器负载高而失败。
  - 修复：`accept` 之后调用 `set_nonblocking(false)`。修复后相关模块连续运行 3 次，全部通过。
- `tests/update_installer.ps1:26`：Windows PowerShell 5.1 没有自动加载 `System.IO.Compression`，`[IO.Compression.ZipArchiveMode]` 找不到类型。改为显式加载 `System.IO.Compression, System.IO.Compression.FileSystem`。CI 用 pwsh 7 运行，原本不受影响，这次改动对 CI 也没有影响。

---

## 4. 刻意未改的内容（不是遗漏）

- `src/updates.rs:5-8`：更新地址仍指向原作者仓库，等确定新仓库地址后再改。
- 关于页里原作者的收款码（`src/extras.rs` 的 `support_author`，`assets/support/`）。
- 界面名称"岛仔"、快捷方式文件名 `岛仔.lnk`、卸载项的显示名、程序文件名 `daozai.exe`：都按"界面名称暂不改"的决定保留。快捷方式与原版同名，后安装的会覆盖先安装的，这一点已知。
- 原版的数据和凭据**不做迁移**：分支使用全新的数据目录和凭据名。
- 高风险的第三方集成（Grok、Gemini、Claude Desktop、读取播放器进程内存）：产品方决定全部保留，本次没有改动。

---

## 5. 验证

环境：Windows 11、Rust 1.99.1、MSVC Build Tools 2022、Windows PowerShell 5.1。

| 项目 | 结果 |
|---|---|
| 改动前基线 `cargo test --locked --all-targets --features record -- --test-threads=1` | 507 通过，0 失败，49 忽略（需要真实账号） |
| 改动后同一命令 | 510 通过；唯一失败的是第 3 节那个不稳定测试，修复后重跑通过 |
| 新增测试 | `llm::tests` 8 个（含 1 个忽略的 `live`），`tray_identifiers_follow_app_id`，`llm_settings_show_every_field_inside_the_small_settings_window` |
| `tests/update_installer.ps1` | success / inherited-shell / bad-hash / wrong-version / traversal 五个场景全部通过 |
| 安装与打包脚本的 PowerShell 语法检查（与 CI 相同） | 通过 |
| `cargo build --locked --release` + `scripts/check-runtime.ps1` | 通过，没有外部 C/C++ 运行库依赖 |
| 编译警告 | 仍是原有的 6 条，没有新增 |
| 连接真实服务：LM Studio 上的 `qwen2-7b-instruct` | `连接成功 · qwen2-7b-instruct · 用时 2.3 秒 · 回复：OK`；模型名错误、端口错误时的中文提示都已验证 |

连接真实服务的测试命令：

```bash
DAOZAI_LLM_URL=http://127.0.0.1:1234/v1 DAOZAI_LLM_MODEL=qwen2-7b-instruct \
  cargo test --lib --features record llm::tests::live -- --ignored --nocapture
```

**未验证**：
- gpt-oss 本身：未连接内网服务，`reasoning_effort` 参数和 `reasoning_content` 字段的处理只用模拟服务器测过；
- 由企业 CA 签发的 HTTPS 证书；
- NSIS 安装器的完整安装、卸载流程（`tests/native_installer.ps1`），本机没有 NSIS。
