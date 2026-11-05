//! 本分支的系统标识，与原版岛仔（`Daozai`）区分，避免两者同时安装时共用数据目录、
//! 凭据、单实例锁和注册表项而互相覆盖。界面上的名称仍为「岛仔」。
//!
//! 改名时：
//! 1. 修改下面的 `app_id!`；
//! 2. 同步 `src/tray.rs` 中的 3 个 `w!()` 常量（`tray_identifiers_follow_app_id` 测试会校验）；
//! 3. 同步 `installer/install.ps1`、`installer/uninstall.ps1` 顶部的 `$appId`。
//!
//! NSIS 安装器的 `APP_ID` 由 `scripts/package.ps1` 从本文件读取；应用内更新脚本由程序传入。

/// 以字面量形式提供标识，便于 `concat!` 拼接常量。
#[macro_export]
macro_rules! app_id {
    () => {
        "DaozaiFork"
    };
}

pub const APP_ID: &str = app_id!();

pub const REPOSITORY: &str = "https://github.com/Oracle0703/lingdongdao";
pub const RELEASES: &str = "https://github.com/Oracle0703/lingdongdao/releases";
pub const RELEASE_API: &str = "https://api.github.com/repos/Oracle0703/lingdongdao/releases/latest";
