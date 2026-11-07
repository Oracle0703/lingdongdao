//! 系统托盘、右键菜单与开机启动。
//!
//! 托盘行为：
//! - `uID=1`，`uFlags=NIF_MESSAGE|NIF_ICON|NIF_TIP`，回调 `WM_APP+9`，提示 `岛仔`；
//! - 图标取 exe 内 `RT_GROUP_ICON #1`，按 `SM_CXSMICON` 走 `LoadImageW`；
//! - 左键、右键都弹同一个菜单，没有双击分支；
//! - 菜单固定中文，含设置入口，前三项每次打开现算勾选态；
//! - 弹菜单前 `SetForegroundWindow`，期间 `SetTimer(12, 16ms)`，收尾 `KillTimer` +
//!   `PostMessage(WM_NULL)` + `DestroyMenu`；
//! - `Exit` 先摘图标再 `ExitProcess`；`TaskbarCreated` 广播时重建图标。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM, ERROR_ALREADY_EXISTS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{CreateMutexW, ExitProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIF_INFO, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 原版的托盘回调消息。
pub const WM_TRAY: u32 = WM_APP + 9;
/// 仅构建并导出当前菜单状态，不弹出菜单（自动化取证用）。
pub const WM_TRAY_DUMP: u32 = WM_APP + 2;
/// 在屏幕坐标 (wparam, lparam) 弹出菜单（隐藏桌面取证用）。
pub const WM_TRAY_MENU: u32 = WM_APP + 3;

const TRAY_UID: u32 = 1;
const CMD_EXIT: u32 = 1;
const CMD_START: u32 = 2;
const CMD_MASCOT: u32 = 3;
const CMD_ABOUT: u32 = 4;
const CMD_MINIMAL: u32 = 5;
const CMD_SETTINGS:u32=6;
const CMD_SCREENSHOT: u32 = 7;
const SCREENSHOT_HOTKEY_ID: i32 = 1;
const TIMER_MENU: usize = 12;
const WM_SET_HOTKEY: u32 = WM_APP + 6;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
// w!() 只接受字面量，需与 brand::APP_ID 保持一致（见 tray_identifiers_follow_app_id 测试）。
const RUN_VALUE: PCWSTR = w!("DaozaiFork");
const MUTEX_NAME: PCWSTR = w!("Local\\DaozaiFork.SingleInstance");
const TRAY_CLASS: PCWSTR = w!("DaozaiForkTray");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub pet: bool,
    pub minimal: bool,
    pub autostart: bool,
    /// 截图页开启时才显示「区域截图」菜单项。
    pub screenshot: bool,
}

/// 截图快捷键预设，与设置里的 `screenshot_hotkey` 键名一一对应（见 `model::SCREENSHOT_HOTKEYS`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenshotHotkey {
    #[default]
    AltA,
    AltS,
    CtrlAltA,
    F1,
    Off,
}

impl ScreenshotHotkey {
    const ALL: [Self; 5] = [Self::AltA, Self::AltS, Self::CtrlAltA, Self::F1, Self::Off];

    pub fn label(self) -> &'static str {
        match self { Self::AltA => "Alt + A", Self::AltS => "Alt + S", Self::CtrlAltA => "Ctrl + Alt + A", Self::F1 => "F1", Self::Off => "关闭" }
    }

    pub fn key(self) -> &'static str {
        match self { Self::AltA => "alt_a", Self::AltS => "alt_s", Self::CtrlAltA => "ctrl_alt_a", Self::F1 => "f1", Self::Off => "off" }
    }

    /// 设置中的键名；未知值按默认 `Alt + A` 处理。
    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|hotkey| hotkey.key() == key).unwrap_or_default()
    }

    fn code(self) -> usize { Self::ALL.iter().position(|hotkey| *hotkey == self).unwrap_or(0) }

    fn from_code(code: usize) -> Option<Self> { Self::ALL.get(code).copied() }

    fn registration(self) -> Option<(HOT_KEY_MODIFIERS, u32)> {
        match self {
            Self::AltA => Some((MOD_ALT | MOD_NOREPEAT, b'A' as u32)),
            Self::AltS => Some((MOD_ALT | MOD_NOREPEAT, b'S' as u32)),
            Self::CtrlAltA => Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, b'A' as u32)),
            Self::F1 => Some((MOD_NOREPEAT, 0x70)),
            Self::Off => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeyStatus {
    pub hotkey: ScreenshotHotkey,
    pub registered: bool,
    pub error_code: Option<u32>,
}

impl Default for HotkeyStatus {
    fn default() -> Self {
        Self { hotkey: ScreenshotHotkey::Off, registered: false, error_code: None }
    }
}

impl HotkeyStatus {
    pub fn message(self) -> String {
        let label = self.hotkey.label();
        if self.hotkey == ScreenshotHotkey::Off { return "快捷键已关闭；仍可使用托盘「区域截图」或截图页的「新截图」。".into(); }
        if self.registered { return format!("{label} 已启用。"); }
        if self.error_code == Some(1409) {
            let hint = if self.hotkey == ScreenshotHotkey::AltA { "（常见是微信截图）" } else { "" };
            format!("{label} 已被其他程序占用{hint}，请换一个快捷键，或关掉占用它的程序后重新选择。")
        } else {
            format!("{label} 注册失败（Windows 错误 {}），可以换一个快捷键再试。", self.error_code.unwrap_or(0))
        }
    }
}

/// 菜单固定中文：启动 / 吉祥物 / 极简 / —— / 设置 / 关于 / 退出。
fn labels() -> [&'static str; 6] {
    ["开机启动", "吉祥物", "极简刘海", "关于岛仔", "退出", "设置…"]
}

struct Ctx {
    tx: Sender<Event>,
    state: Arc<Mutex<State>>,
    taskbar_msg: u32,
    hicon: HICON,
    added: AtomicBool,
    adds: AtomicU32,
    hotkey: Arc<Mutex<HotkeyStatus>>,
}

impl Ctx {
    fn state(&self) -> State {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hotkey_status(&self) -> HotkeyStatus {
        *self.hotkey.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 只从托盘窗口线程调用。失败时不抢占、不自动回退。
    fn change_hotkey(&self, hwnd: HWND, hotkey: ScreenshotHotkey) {
        let status = register_screenshot_hotkey(hwnd, self.hotkey_status(), hotkey);
        *self.hotkey.lock().unwrap_or_else(|e| e.into_inner()) = status;
        let _ = self.tx.send(Event::ScreenshotHotkeyChanged(status));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    TogglePet,
    OpenSettings,
    OpenCalendar,
    ToggleMinimal,
    ToggleAutostart,
    StartScreenshot,
    ScreenshotHotkeyChanged(HotkeyStatus),
}

fn register_screenshot_hotkey(hwnd: HWND, old: HotkeyStatus, hotkey: ScreenshotHotkey) -> HotkeyStatus {
    if old.registered {
        if let Err(error) = unsafe { UnregisterHotKey(hwnd, SCREENSHOT_HOTKEY_ID) } {
            // 注销失败时保留真实的旧状态，不再叠加新的注册。
            eprintln!("screenshot hotkey: 注销 {} 失败：{error}", old.hotkey.label());
            return old;
        }
    }
    let mut status = HotkeyStatus { hotkey, registered: false, error_code: None };
    if let Some((modifiers, key)) = hotkey.registration() {
        match unsafe { RegisterHotKey(hwnd, SCREENSHOT_HOTKEY_ID, modifiers, key) } {
            Ok(()) => status.registered = true,
            Err(error) => status.error_code = Some(error.code().0 as u32 & 0xffff),
        }
    }
    status
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fill_tip(dst: &mut [u16], s: &str) {
    let src = wide(s);
    let n = (dst.len() - 1).min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

/// exe 里编入的 `RT_GROUP_ICON #1`，按系统小图标尺寸取最合适的一帧。
fn load_resource_icon() -> Option<HICON> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let cx = GetSystemMetrics(SM_CXSMICON);
        let handle = LoadImageW(module, PCWSTR(1 as *const u16), IMAGE_ICON, cx, cx, LR_DEFAULTCOLOR).ok()?;
        (!handle.is_invalid()).then_some(HICON(handle.0))
    }
}

/// 图标取不到时，直接解析内嵌的岛仔 ICO。
fn load_embedded_icon() -> Option<HICON> {
    let ico: &[u8] = include_bytes!("../assets/daozai.ico");
    if ico.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut best: Option<(i32, u32, u32)> = None;
    for i in 0..count {
        let at = 6 + i * 16;
        if at + 16 > ico.len() {
            break;
        }
        let entry = &ico[at..at + 16];
        let w = if entry[0] == 0 { 256 } else { entry[0] as i32 };
        let bytes = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
        let off = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);
        if off as usize + bytes as usize > ico.len() {
            continue;
        }
        let score = (w - 32).abs();
        if best.is_none_or(|(s, _, _)| score < s) {
            best = Some((score, bytes, off));
        }
    }
    let (_, bytes, off) = best?;
    let data = &ico[off as usize..(off + bytes) as usize];
    unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, 0, 0, LR_DEFAULTCOLOR).ok() }
}

fn load_icon() -> Option<HICON> {
    load_resource_icon().or_else(load_embedded_icon)
}

fn add_icon(hwnd: HWND, hicon: HICON) -> bool {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: hicon,
        ..Default::default()
    };
    fill_tip(&mut nid.szTip, "岛仔");
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            // explorer 里已有同名残留时，用 MODIFY 顶掉
            return Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool();
        }
        true
    }
}

fn remove_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn popup_menu(state: State, hotkey: HotkeyStatus) -> windows::core::Result<HMENU> {
    let text = labels();
    let wide: Vec<Vec<u16>> = text.iter().map(|s| wide(s)).collect();
    unsafe {
        let menu = CreatePopupMenu()?;
        let append = |flags: MENU_ITEM_FLAGS, id: u32, label: PCWSTR| {
            let _ = AppendMenuW(menu, flags, id as usize, label);
        };
        let check = |on: bool| {
            if on {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            }
        };
        append(check(state.autostart), CMD_START, PCWSTR(wide[0].as_ptr()));
        append(check(state.pet), CMD_MASCOT, PCWSTR(wide[1].as_ptr()));
        append(check(state.minimal), CMD_MINIMAL, PCWSTR(wide[2].as_ptr()));
        append(MF_SEPARATOR, 0, PCWSTR::null());
        if state.screenshot {
            // 制表符后的文字由菜单右对齐，显示已生效的快捷键。
            let label = if hotkey.registered { self::wide(&format!("区域截图\t{}", hotkey.hotkey.label().replace(' ', ""))) } else { self::wide("区域截图") };
            append(MF_STRING, CMD_SCREENSHOT, PCWSTR(label.as_ptr()));
        }
        append(MF_STRING, CMD_SETTINGS, PCWSTR(wide[5].as_ptr()));
        append(MF_STRING, CMD_ABOUT, PCWSTR(wide[3].as_ptr()));
        append(MF_STRING, CMD_EXIT, PCWSTR(wide[4].as_ptr()));
        Ok(menu)
    }
}

fn menu_items(menu: HMENU) -> Vec<(String, bool, bool)> {
    unsafe {
        let count = GetMenuItemCount(menu);
        (0..count)
            .map(|i| {
                let mut buf = [0u16; 128];
                let flags = GetMenuState(menu, i as u32, MF_BYPOSITION);
                let sep = flags & MF_SEPARATOR.0 != 0;
                let len = if sep {
                    0
                } else {
                    GetMenuStringW(menu, i as u32, Some(&mut buf), MF_BYPOSITION)
                };
                let text = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
                (text, flags & MF_CHECKED.0 != 0, sep)
            })
            .collect()
    }
}

fn about() {
    let text = wide(&format!(
        "岛仔 {}\n\nWindows 桌面灵动岛小助手\n退出请右键托盘图标，选择「退出」。",
        env!("CARGO_PKG_VERSION")
    ));
    let title = wide("关于岛仔");
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn dump_menu(hwnd: HWND, ctx: &Ctx, menu: HMENU) {
    if let Ok(path) = std::env::var("DAOZAI_TRAY_DUMP") {
        let state = ctx.state();
        let mut body = String::new();
        body.push_str(&format!("hwnd=0x{:X}\n", hwnd.0 as usize));
        body.push_str(&format!("icon_added={}\n", ctx.added.load(Ordering::Relaxed)));
        body.push_str(&format!("icon_adds={}\n", ctx.adds.load(Ordering::Relaxed)));
        body.push_str(&format!("autostart={}\n", state.autostart));
        body.push_str(&format!("pet={}\nminimal={}\n", state.pet, state.minimal));
        let hotkey = ctx.hotkey_status();
        body.push_str(&format!("screenshot_hotkey={}\nscreenshot_hotkey_registered={}\nscreenshot_hotkey_error={:?}\n", hotkey.hotkey.label(), hotkey.registered, hotkey.error_code));
        for (i, (text, checked, sep)) in menu_items(menu).into_iter().enumerate() {
            if sep {
                body.push_str(&format!("item{}=(separator)\n", i + 1));
            } else {
                body.push_str(&format!("item{}={}|checked={}\n", i + 1, text, u8::from(checked)));
            }
        }
        let _ = std::fs::write(path, body);
    }
}

fn show_menu_at(hwnd: HWND, ctx: &Ctx, x: i32, y: i32) {
    let Ok(menu) = popup_menu(ctx.state(), ctx.hotkey_status()) else { return };
    dump_menu(hwnd, ctx, menu);
    unsafe {
        let previous_foreground = GetForegroundWindow();
        // 不先给前台，菜单会因为焦点切换立刻消失
        let _ = SetForegroundWindow(hwnd);
        let _ = SetTimer(hwnd, TIMER_MENU, 16, None);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            x,
            y,
            0,
            hwnd,
            None,
        )
        .0 as u32;
        let _ = KillTimer(hwnd, TIMER_MENU);
        // 收尾：让菜单彻底消失
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd {
            CMD_START => {
                let _ = ctx.tx.send(Event::ToggleAutostart);
            }
            CMD_MASCOT => {
                let _ = ctx.tx.send(Event::TogglePet);
            }
            CMD_MINIMAL => {
                let _ = ctx.tx.send(Event::ToggleMinimal);
            }
            CMD_SETTINGS => {let _=ctx.tx.send(Event::OpenSettings);}
            CMD_SCREENSHOT => {
                // 截图线程稍后记录前台窗口；避免把临时托盘窗口当成归还焦点的目标。
                // 若用户已经切换到了别处，则不抢回焦点。
                if previous_foreground != hwnd && IsWindow(previous_foreground).as_bool() && GetForegroundWindow() == hwnd {
                    let _ = SetForegroundWindow(previous_foreground);
                }
                let _ = ctx.tx.send(Event::StartScreenshot);
            }
            CMD_ABOUT => about(),
            CMD_EXIT => {
                // 原版：先摘图标，再结束进程
                remove_icon(hwnd);
                let _ = UnregisterHotKey(hwnd, SCREENSHOT_HOTKEY_ID);
                ExitProcess(0);
            }
            _ => {}
        }
    }
}

fn show_menu(hwnd: HWND, ctx: &Ctx) {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        show_menu_at(hwnd, ctx, p.x, p.y);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ctx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ctx;
    if ctx_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ctx = &*ctx_ptr;
    if msg == WM_TRAY {
        let mouse = (lp.0 as u32) & 0xffff;
        // 原版左右键同一个分支，没有双击行为
        if matches!(mouse, WM_LBUTTONUP | WM_RBUTTONUP) {
            show_menu(hwnd, ctx);
        }
        return LRESULT(0);
    }
    if msg == WM_APP+4 {return LRESULT(ctx.tx.send(Event::OpenSettings).is_ok() as isize);}
    if msg == WM_APP+5 {return LRESULT(ctx.tx.send(Event::OpenCalendar).is_ok() as isize);}
    // 主线程改了设置：RegisterHotKey 必须由创建窗口的线程调用，所以经消息转到托盘线程。
    if msg == WM_SET_HOTKEY {
        if let Some(hotkey) = ScreenshotHotkey::from_code(wp.0) { ctx.change_hotkey(hwnd, hotkey); }
        return LRESULT(0);
    }
    if msg == WM_HOTKEY && wp.0 == SCREENSHOT_HOTKEY_ID as usize {
        let status = ctx.hotkey_status();
        // 旧组合键的排队消息在切换后失效；MOD_NOREPEAT 不在 WM_HOTKEY 的低字中。
        let modifiers = lp.0 as u32 & 0xffff;
        let key = (lp.0 as u32 >> 16) & 0xffff;
        if status.registered && status.hotkey.registration().is_some_and(|(mods, vk)| key == vk && modifiers == (mods.0 & !MOD_NOREPEAT.0)) {
            let _ = ctx.tx.send(Event::StartScreenshot);
        }
        return LRESULT(0);
    }
    if msg == WM_TRAY_DUMP {
        if let Ok(menu) = popup_menu(ctx.state(), ctx.hotkey_status()) {
            dump_menu(hwnd, ctx, menu);
            unsafe {
                let _ = DestroyMenu(menu);
            }
        }
        return LRESULT(0);
    }
    if msg == WM_TRAY_MENU {
        show_menu_at(hwnd, ctx, wp.0 as i32, lp.0 as i32);
        return LRESULT(0);
    }
    if ctx.taskbar_msg != 0 && msg == ctx.taskbar_msg {
        let added = add_icon(hwnd, ctx.hicon);
        ctx.added.store(added, Ordering::Relaxed);
        ctx.adds.fetch_add(1, Ordering::Relaxed);
        return LRESULT(0);
    }
    match msg {
        WM_CLOSE => {
            let _ = UnregisterHotKey(hwnd, SCREENSHOT_HOTKEY_ID);
            remove_icon(hwnd);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub struct Tray {
    hwnd: HWND,
    events: Receiver<Event>,
    state: Arc<Mutex<State>>,
    hotkey: Arc<Mutex<HotkeyStatus>>,
    _thread: JoinHandle<()>,
}

impl Tray {
    /// `initial_hotkey`：按设置注册的截图快捷键；截图页关闭时传 `Off`。
    pub fn start(state: State, initial_hotkey: ScreenshotHotkey) -> Option<Self> {
        let shared = Arc::new(Mutex::new(state));
        let hotkey = Arc::new(Mutex::new(HotkeyStatus::default()));
        let (event_tx, event_rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let thread_state = shared.clone();
        let thread_hotkey = hotkey.clone();
        let thread = std::thread::Builder::new()
            .name("daozai-tray".into())
            .spawn(move || unsafe {
                let Ok(module) = GetModuleHandleW(None) else {
                    let _ = ready_tx.send(Err("GetModuleHandleW 失败".to_string()));
                    return;
                };
                let hinst = HINSTANCE(module.0);
                let class = TRAY_CLASS;
                let wc = WNDCLASSW {
                    style: WNDCLASS_STYLES(0),
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: HICON::default(),
                    hCursor: HCURSOR::default(),
                    hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
                    lpszMenuName: PCWSTR::null(),
                    lpszClassName: class,
                };
                let _ = RegisterClassW(&wc);
                let hwnd = match CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class,
                    w!("岛仔托盘"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    hinst,
                    None,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("创建托盘窗口失败：{e}")));
                        return;
                    }
                };
                let Some(hicon) = load_icon() else {
                    let _ = ready_tx.send(Err("读取托盘图标失败".to_string()));
                    return;
                };
                let ctx = Box::new(Ctx {
                    tx: event_tx,
                    state: thread_state,
                    taskbar_msg: RegisterWindowMessageW(w!("TaskbarCreated")),
                    hicon,
                    added: AtomicBool::new(false),
                    adds: AtomicU32::new(0),
                    hotkey: thread_hotkey,
                });
                let ctx_ptr = Box::into_raw(ctx);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
                let added = add_icon(hwnd, hicon);
                (*ctx_ptr).added.store(added, Ordering::Relaxed);
                (*ctx_ptr).adds.store(1, Ordering::Relaxed);
                if initial_hotkey != ScreenshotHotkey::Off { (*ctx_ptr).change_hotkey(hwnd, initial_hotkey); }
                let _ = ready_tx.send(Ok((hwnd.0 as isize, added)));

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                remove_icon(hwnd);
                let _ = UnregisterHotKey(hwnd, SCREENSHOT_HOTKEY_ID);
                let _ = DestroyIcon(hicon);
                drop(Box::from_raw(ctx_ptr));
            });
        let thread = match thread {
            Ok(t) => t,
            Err(_) => return None,
        };
        let (hwnd, _added) = match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                eprintln!("tray: {e}");
                return None;
            }
            Err(_) => {
                eprintln!("tray: 启动超时");
                return None;
            }
        };
        Some(Self { hwnd: HWND(hwnd as *mut _), events: event_rx, state: shared, hotkey, _thread: thread })
    }

    #[allow(dead_code)]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 设置改变时调用；实际注册在托盘线程完成，结果经 `Event::ScreenshotHotkeyChanged` 返回。
    pub fn set_screenshot_hotkey(&self, hotkey: ScreenshotHotkey) {
        unsafe { let _ = PostMessageW(self.hwnd, WM_SET_HOTKEY, WPARAM(hotkey.code()), LPARAM(0)); }
    }

    pub fn hotkey_status(&self) -> HotkeyStatus {
        *self.hotkey.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn notify(&self,title:&str,body:&str,sound:bool) {
        let mut nid=NOTIFYICONDATAW {cbSize:std::mem::size_of::<NOTIFYICONDATAW>() as u32,hWnd:self.hwnd,uID:TRAY_UID,uFlags:NIF_INFO,dwInfoFlags:NIIF_INFO,..Default::default()};
        fill_tip(&mut nid.szInfoTitle,title);fill_tip(&mut nid.szInfo,body);
        // NIIF_NOSOUND，默认安静提醒。
        if !sound {nid.dwInfoFlags.0|=0x10;}
        unsafe{let _=Shell_NotifyIconW(NIM_MODIFY,&nid);}
    }
    pub fn sync(&self, state: State) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *guard != state {
            *guard = state;
        }
    }

    pub fn poll(&self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = self.events.try_recv() {
            out.push(e);
        }
        out
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// 单实例：已有岛仔实例时返回 false。
pub fn single_instance() -> bool {
    unsafe {
        match CreateMutexW(None, false, MUTEX_NAME) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(handle);
                    false
                } else {
                    // HANDLE 没有 Drop，故意不 CloseHandle：进程存活期间一直持有互斥体
                    let _ = handle;
                    true
                }
            }
            Err(_) => true,
        }
    }
}

fn run_key() -> Option<HKEY> {
    unsafe {
        let mut key = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        (rc.0 == 0).then_some(key)
    }
}

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_QUERY_VALUE, &mut key).0 != 0 {
            return false;
        }
        let mut size = 0u32;
        let rc = RegQueryValueExW(key, RUN_VALUE, None, None, None, Some(&mut size));
        let _ = RegCloseKey(key);
        rc.0 == 0 && size > 0
    }
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = format!("\"{}\"", exe.display());
            let utf16 = wide(&value);
            let bytes = std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2);
            let key = run_key().ok_or_else(|| "打不开 HKCU\\Run".to_string())?;
            let rc = RegSetValueExW(key, RUN_VALUE, 0, REG_SZ, Some(bytes));
            let _ = RegCloseKey(key);
            if rc.0 != 0 {
                return Err(format!("写 HKCU\\Run 失败：{}", rc.0));
            }
        } else {
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key).0 == 0 {
                let _ = RegDeleteValueW(key, RUN_VALUE);
                let _ = RegCloseKey(key);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_identifiers_follow_app_id() {
        let text=|value:PCWSTR|unsafe{value.to_string()}.unwrap();
        let id=crate::brand::APP_ID;
        assert_eq!(text(RUN_VALUE),id);
        assert_eq!(text(MUTEX_NAME),format!("Local\\{id}.SingleInstance"));
        assert_eq!(text(TRAY_CLASS),format!("{id}Tray"));
    }

    #[test]
    fn main_window_lookup_requires_the_matching_process(){unsafe{
        let window=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!("岛仔"),WS_POPUP,0,0,1,1,None,None,None,None).unwrap();
        let pid=windows::Win32::System::Threading::GetCurrentProcessId();
        assert_eq!(main_window_for_process(pid),Some(window));
        assert!(main_window_for_process(u32::MAX).is_none());
        assert!(main_window_for_process(0).is_none());
        DestroyWindow(window).unwrap();
    }}

    #[test]
    fn repeated_launch_requests_settings_or_calendar() {
        // 普通回归不得临时抢占用户的全局快捷键。
        let tray=Tray::start(State::default(), ScreenshotHotkey::Off).expect("托盘应能启动");
        for (calendar,expected) in [(false,Event::OpenSettings),(true,Event::OpenCalendar)] {
            assert!(request_existing_window(tray.hwnd(),calendar));
            assert_eq!(tray.events.recv_timeout(Duration::from_secs(2)).unwrap(),expected);
        }
        assert!(!request_existing_window(HWND::default(),false));
    }

    #[test]
    fn menu_order_checks_and_separator() {
        let registered = HotkeyStatus { hotkey: ScreenshotHotkey::AltA, registered: true, error_code: None };
        for (state, want, hotkey, screenshot_item) in [
            (State { pet: false, minimal: false, autostart: false, screenshot: true }, [false, false, false], HotkeyStatus::default(), Some("区域截图")),
            (State { pet: true, minimal: false, autostart: true, screenshot: true }, [true, true, false], registered, Some("区域截图\tAlt+A")),
            (State { pet: false, minimal: true, autostart: false, screenshot: false }, [false, false, true], registered, None),
        ] {
            let menu = popup_menu(state, hotkey).expect("创建菜单");
            let items = menu_items(menu);
            assert!(items[3].2, "第 4 项是分隔线");
            let texts: Vec<_> = items.iter().filter(|i| !i.2).map(|i| i.0.as_str()).collect();
            let mut expected = vec!["开机启动", "吉祥物", "极简刘海"];
            expected.extend(screenshot_item);
            expected.extend(["设置…", "关于岛仔", "退出"]);
            assert_eq!(texts, expected, "截图页关闭时不显示「区域截图」");
            assert_eq!([items[0].1, items[1].1, items[2].1], want, "勾选态应与状态一致");
            unsafe {
                let _ = DestroyMenu(menu);
            }
        }
    }

    #[test]
    fn labels_are_fixed_chinese() {
        assert_eq!(labels(), ["开机启动", "吉祥物", "极简刘海", "关于岛仔", "退出", "设置…"]);
    }

    #[test]
    fn hotkey_error_message_does_not_claim_every_error_is_a_conflict() {
        let conflict = HotkeyStatus { hotkey: ScreenshotHotkey::AltA, registered: false, error_code: Some(1409) };
        assert!(conflict.message().contains("微信"));
        assert!(conflict.message().contains("已被其他程序占用"));
        let failure = HotkeyStatus { error_code: Some(5), ..conflict };
        assert!(failure.message().contains("Windows 错误 5"));
        assert!(!failure.message().contains("已被其他程序占用"));
        assert_eq!(ScreenshotHotkey::default(), ScreenshotHotkey::AltA);
        assert_eq!(ScreenshotHotkey::AltS.registration(), Some((MOD_ALT | MOD_NOREPEAT, b'S' as u32)));
        assert_eq!(ScreenshotHotkey::CtrlAltA.registration(), Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, b'A' as u32)));
        assert_eq!(ScreenshotHotkey::F1.registration(), Some((MOD_NOREPEAT, 0x70)));
        assert!(ScreenshotHotkey::Off.registration().is_none());
        for hotkey in ScreenshotHotkey::ALL {
            assert_eq!(ScreenshotHotkey::from_key(hotkey.key()), hotkey);
            assert_eq!(ScreenshotHotkey::from_code(hotkey.code()), Some(hotkey));
        }
        assert_eq!(ScreenshotHotkey::from_key("未知"), ScreenshotHotkey::AltA, "未知键名按默认处理");
        let keys: Vec<_> = ScreenshotHotkey::ALL.iter().map(|hotkey| hotkey.key()).collect();
        let offered: Vec<_> = crate::model::SCREENSHOT_HOTKEYS.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, offered, "设置页的选项与托盘注册的预设一致");
    }

    #[test]
    #[ignore = "需人工确认 Alt+S 可临时注册：验证真实 Win32 全局热键冲突与释放"]
    fn screenshot_hotkey_native_conflict_and_release() {
        unsafe {
            // 两个不同 HWND 的同组合键注册必须冲突；不覆盖用户既有热键。
            let make_window = || CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("截图热键测试"), WS_POPUP, 0, 0, 1, 1, None, None, None, None).unwrap();
            let first = make_window();
            let second = make_window();
            struct Windows(HWND, HWND);
            impl Drop for Windows {
                fn drop(&mut self) { unsafe { for window in [self.0, self.1] { let _ = UnregisterHotKey(window, SCREENSHOT_HOTKEY_ID); let _ = DestroyWindow(window); } } }
            }
            let _windows = Windows(first, second);
            let initial = register_screenshot_hotkey(first, HotkeyStatus::default(), ScreenshotHotkey::AltS);
            assert!(initial.registered, "Alt+S 当前不可用，未更改任何其他软件注册：{}", initial.message());
            let blocked = register_screenshot_hotkey(second, HotkeyStatus::default(), ScreenshotHotkey::AltS);
            assert_eq!(blocked.error_code, Some(1409));
            assert!(!blocked.registered);
            let off = register_screenshot_hotkey(first, initial, ScreenshotHotkey::Off);
            assert_eq!(off, HotkeyStatus::default());
            let retry = register_screenshot_hotkey(second, blocked, ScreenshotHotkey::AltS);
            assert!(retry.registered, "注销后应可注册：{}", retry.message());
        }
    }

    #[test]
    fn icon_resource_is_embedded() {
        // 托盘图标必须来自 exe 资源 #1（rc.exe 编入），不是临时画的
        assert!(load_resource_icon().is_some(), "RT_GROUP_ICON #1 取不到");
    }

    #[test]
    fn embedded_icon_still_parsable_as_fallback() {
        let ico: &[u8] = include_bytes!("../assets/daozai.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert!(load_embedded_icon().is_some());
    }

    #[test]
    fn menu_ids_match_original() {
        assert_eq!(
            [CMD_EXIT, CMD_START, CMD_MASCOT, CMD_ABOUT, CMD_MINIMAL],
            [1, 2, 3, 4, 5]
        );
    }
}

/// 重复启动时请求现有实例打开窗口，限制等待时间以免跟随挂起。
fn request_existing_window(hwnd:HWND,calendar:bool)->bool {unsafe{
    if hwnd.0.is_null(){return false;}
    let mut reply=0usize;
    SendMessageTimeoutW(hwnd,if calendar{WM_APP+5}else{WM_APP+4},WPARAM(0),LPARAM(0),
        SMTO_ABORTIFHUNG|SMTO_ERRORONEXIT,1500,Some(&mut reply)).0!=0 && reply==1
}}

pub fn activate_existing(calendar:bool)->bool {unsafe{
    let Ok(tray)=FindWindowW(TRAY_CLASS,PCWSTR::null()) else{return false;};
    let mut tray_pid=0;
    GetWindowThreadProcessId(tray,Some(&mut tray_pid));
    let Some(window)=main_window_for_process(tray_pid) else{return false;};
    // 托盘有独立线程，能接收消息不代表界面线程仍可响应。
    if SendMessageTimeoutW(window,WM_NULL,WPARAM(0),LPARAM(0),
        SMTO_ABORTIFHUNG|SMTO_ERRORONEXIT,1500,None).0==0{return false;}
    request_existing_window(tray,calendar)
}}

// 原版与分支可以使用相同显示名称；只查找本分支托盘进程所属的主窗口。
fn main_window_for_process(pid:u32)->Option<HWND> {
    if pid==0{return None;}
    struct Search {pid:u32,window:Option<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,data:LPARAM)->BOOL {
        let search=&mut *(data.0 as *mut Search);let mut owner=0;
        GetWindowThreadProcessId(hwnd,Some(&mut owner));
        if owner==search.pid {
            let mut title=[0u16;64];let len=GetWindowTextW(hwnd,&mut title) as usize;
            if String::from_utf16_lossy(&title[..len])=="岛仔"{search.window=Some(hwnd);return BOOL(0);}
        }
        BOOL(1)
    }
    let mut search=Search{pid,window:None};
    unsafe{let _=EnumWindows(Some(visit),LPARAM(&mut search as *mut Search as isize));}
    search.window
}
