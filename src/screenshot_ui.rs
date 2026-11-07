//! 截图：岛上的「截图」页与截图后自动展开的预览（同一套图库绘制），以及设置窗口里的「截图」分组。
//!
//! 版式（逻辑点，面板 419 × 246）：标题行 0–36；主预览 16–403 × 38–168；缩略图条 174–202；
//! 操作按钮 208–230；底部页码豆由 `nav` 绘制。本页不画左右翻页箭头，左右箭头只用于切换截图。
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::{App, Cmd, Env, SCREENSHOT_PAGE};
use crate::extras::{setting_row, settings_card, settings_control_row, settings_label};
use crate::model::SCREENSHOT_HOTKEYS;
use crate::paint::{self, Card, H};
use crate::screenshot::library::{self, Images, Library, Request, Response};
use crate::screenshot::store::{self, Entry};
use crate::settings_theme::color;
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, Pos2, Rect, RichText};

/// 设置窗口中截图分组的编号。
pub(crate) const SETTINGS_TAB: usize = 14;
/// 截图页与截图预览的面板高度。
pub(crate) const HEIGHT: f32 = 246.0;
const MAIN: (f32, f32, f32, f32) = (16.0, 38.0, 387.0, 130.0);
const THUMB_Y: f32 = 174.0;
const THUMB: (f32, f32, f32) = (46.0, 28.0, 6.0);
const ACTION_Y: f32 = 208.0;
/// 拖动方向在位移达到这么多逻辑点时判定一次；横向切图需累计超过 `SWIPE` 才切换。
const DECIDE: f32 = 12.0;
const SWIPE: f32 = 32.0;
/// 纹理只保留当前选中项前后这么多张，其余释放。
const KEEP: usize = 3;

enum Slot {
    Ready { width: u32, height: u32, preview: egui::TextureHandle, thumb: egui::TextureHandle },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode { Undecided, Swipe, Out }

#[derive(Clone, Copy)]
struct MainDrag { start: Pos2, dx: f32, mode: DragMode }

/// 截图后自动展开的预览（覆盖层，不改变 `App::page`）。
pub(crate) struct Preview {
    /// 未悬停时到这个时刻收起；鼠标进入后清空，离开时按现有离开缓冲收起。
    pub until: Option<f64>,
    pub copy: Option<Result<(), String>>,
    /// 悬停过之后离开岛的时刻，用于离开缓冲。
    pub left_at: Option<f64>,
}

#[derive(Default)]
pub(crate) struct Gallery {
    library: Option<Library>,
    /// 当前列表来自哪个目录；与设置不同时重新扫描。
    root: Option<PathBuf>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: usize,
    scanning: bool,
    scanned: bool,
    error: Option<String>,
    generation: u64,
    slots: HashMap<PathBuf, Slot>,
    pending: HashSet<PathBuf>,
    /// 页面上一次绘制的时刻；隔了一段时间再进入截图页时重新扫描，反映外部的增删。
    drawn_at: f64,
    drag: Option<MainDrag>,
    confirm_delete: Option<PathBuf>,
    swipe_at: f64,
    pub(crate) preview: Option<Preview>,
}

impl Gallery {
    pub(crate) fn current(&self) -> Option<&Entry> { self.entries.get(self.selected) }

    /// 测试用：直接进入删除确认状态。
    #[cfg(test)]
    pub(crate) fn ask_delete(&mut self) { self.confirm_delete = self.current().map(|entry| entry.path.clone()); }

    /// 已解码可显示的张数（测试用）。
    #[cfg(test)]
    pub(crate) fn loaded(&self) -> usize { self.slots.values().filter(|slot| matches!(slot, Slot::Ready { .. })).count() }

    pub(crate) fn step(&mut self, delta: isize) -> bool {
        let Some(last) = self.entries.len().checked_sub(1) else { return false };
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        if next == self.selected { return false; }
        self.selected = next;
        self.confirm_delete = None;
        true
    }

    fn select(&mut self, index: usize) {
        if index < self.entries.len() && index != self.selected { self.selected = index; self.confirm_delete = None; }
    }

    fn library(&mut self, ctx: &egui::Context) -> &Library {
        self.library.get_or_insert_with(|| { let ctx = ctx.clone(); Library::start(move || ctx.request_repaint()) })
    }

    fn rescan(&mut self, ctx: &egui::Context, root: Option<PathBuf>) {
        if root != self.root {
            self.generation += 1;
            self.entries.clear();
            self.selected = 0;
            self.slots.clear();
            self.pending.clear();
            self.scanned = false;
            if let Some(library) = &self.library { library.clear_decodes(); }
        }
        self.root = root.clone();
        self.error = None;
        let Some(root) = root else { self.scanning = false; self.scanned = true; return };
        self.scanning = true;
        let generation = self.generation;
        self.library(ctx).request(Request::Scan { root, generation });
    }

    /// 处理后台结果；每帧调用，开销只是一次 `try_iter`。
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let Some(library) = &self.library else { return };
        for response in library.poll() {
            match response {
                Response::Scanned { root, generation, result } if generation == self.generation && Some(&root) == self.root.as_ref() => {
                    self.scanning = false;
                    self.scanned = true;
                    match result {
                        Ok(entries) => {
                            let keep = self.current().map(|entry| entry.path.clone());
                            self.entries = entries;
                            self.selected = keep.and_then(|path| self.entries.iter().position(|entry| entry.path == path)).unwrap_or(0);
                            let listed: HashSet<&PathBuf> = self.entries.iter().map(|entry| &entry.path).collect();
                            self.slots.retain(|path, _| listed.contains(path));
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Response::Decoded { path, generation, result } if generation == self.generation => {
                    self.pending.remove(&path);
                    match result {
                        Ok(images) => { let slot = upload(ctx, &path, &images); self.slots.insert(path, slot); }
                        // 外部删掉的文件直接从列表去掉，不显示失效占位。
                        Err(error) if error == "文件已移动或删除" => self.forget(&path),
                        Err(error) => { self.slots.insert(path, Slot::Failed(error)); }
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn forget(&mut self, path: &Path) {
        let Some(index) = self.entries.iter().position(|entry| entry.path == path) else { return };
        self.entries.remove(index);
        self.slots.remove(path);
        if index < self.selected || self.selected >= self.entries.len() { self.selected = self.selected.saturating_sub(1); }
        if self.confirm_delete.as_deref() == Some(path) { self.confirm_delete = None; }
    }

    /// 请求当前项前后的解码，释放远处的纹理。
    fn ensure_images(&mut self, ctx: &egui::Context) {
        if self.entries.is_empty() { return; }
        let near = self.selected.saturating_sub(KEEP)..=(self.selected + KEEP).min(self.entries.len() - 1);
        let wanted: Vec<PathBuf> = near.clone().map(|index| self.entries[index].path.clone()).collect();
        self.slots.retain(|path, _| wanted.contains(path));
        // 先请求当前项，再由近及远；队列后进先出，所以倒序提交。
        let mut order: Vec<usize> = near.collect();
        order.sort_by_key(|index| std::cmp::Reverse(index.abs_diff(self.selected)));
        for index in order {
            let path = self.entries[index].path.clone();
            if self.slots.contains_key(&path) || self.pending.contains(&path) { continue; }
            self.pending.insert(path.clone());
            let generation = self.generation;
            self.library(ctx).request(Request::Decode { path, generation });
        }
    }

    /// 刚保存的截图：插到最前并选中，直接用内存像素生成的小图，不再解码。
    pub(crate) fn add_saved(&mut self, ctx: &egui::Context, path: &Path, images: &Images) {
        let listed_here = self.root.as_ref().is_some_and(|root| path.parent().and_then(Path::parent) == Some(root.as_path()));
        let Some(key) = path.file_name().and_then(|n| n.to_str()).and_then(store::parse_name) else { return };
        if !listed_here { return; }
        self.entries.retain(|entry| entry.path != path);
        self.entries.insert(0, Entry { path: path.to_path_buf(), key });
        self.selected = 0;
        self.confirm_delete = None;
        self.slots.insert(path.to_path_buf(), upload(ctx, path, images));
    }
}

fn upload(ctx: &egui::Context, path: &Path, images: &Images) -> Slot {
    Slot::Ready {
        width: images.width,
        height: images.height,
        preview: ctx.load_texture(library::texture_name(path, "preview"), images.preview.clone(), library::texture_options()),
        thumb: ctx.load_texture(library::texture_name(path, "thumb"), images.thumb.clone(), library::texture_options()),
    }
}

fn image(c: &Card, texture: &egui::TextureHandle, rect: (f32, f32, f32, f32), radius: f32, tint: Color32) {
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let shape = egui::epaint::RectShape::filled(c.rect(rect.0, rect.1, rect.2, rect.3), radius * c.s, tint).with_texture(texture.id(), uv);
    c.painter.add(shape);
}

/// 等比放进框内并居中。
fn fit(size: (u32, u32), (x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let scale = (w / size.0.max(1) as f32).min(h / size.1.max(1) as f32);
    let (iw, ih) = (size.0 as f32 * scale, size.1 as f32 * scale);
    (x + (w - iw) / 2.0, y + (h - ih) / 2.0, iw, ih)
}

/// `今天 15:30` 或 `11月6日 15:30`。
fn when(key: store::Key, today: &str) -> String {
    let digits = format!("{:017}", key.stamp);
    let (date, hm) = (&digits[..8], format!("{}:{}", &digits[8..10], &digits[10..12]));
    if today.replace('-', "") == date { return format!("今天 {hm}"); }
    let month: u32 = digits[4..6].parse().unwrap_or(0);
    let day: u32 = digits[6..8].parse().unwrap_or(0);
    format!("{month}月{day}日 {hm}")
}

fn button(c: &Card, x: f32, w: f32, label: &str, id: &str, danger: bool, enabled: bool) -> egui::Response {
    let response = c.hit(x, ACTION_Y, w, 22.0, ("shot-action", id));
    let (hover, press) = c.hover_t(&response);
    let hover = if enabled { hover } else { 0.0 };
    c.fill(x + press, ACTION_Y + press, w - press * 2.0, 22.0 - press * 2.0, 11.0, paint::mix(theme::BTN, theme::BTN_HOVER, hover));
    let ink = if !enabled { theme::DIM } else if danger { paint::mix(theme::GRAY, theme::RED_TOP, hover) } else { paint::mix(theme::GRAY, theme::WHITE, hover) };
    c.text(x + w / 2.0, ACTION_Y + 15.0, label, size::SMALL, W::Semibold, ink, H::Center);
    response
}

impl App {
    /// 截图后的自动预览是否正在显示（外壳据此临时取消全屏隐藏）。
    pub fn screenshot_preview_active(&self) -> bool { self.shots.preview.is_some() }

    /// 每帧处理图库的后台结果。
    pub(crate) fn poll_screenshots(&mut self, ctx: &egui::Context) {
        self.shots.poll(ctx);
    }

    /// 截图页：入口、浏览和操作。
    pub(crate) fn page_screenshot(&mut self, c: &Card, env: &Env, cmds: &mut Vec<Cmd>) {
        let ctx = c.ui.ctx().clone();
        let root = store::resolve_root(&self.settings.screenshot_dir);
        // 隔一段时间再进入本页，或保存位置改了：重新扫描，反映资源管理器里的增删。
        if root != self.shots.root || !self.shots.scanned && !self.shots.scanning || self.t - self.shots.drawn_at > 0.5 {
            self.shots.rescan(&ctx, root.clone());
        }
        self.shots.drawn_at = self.t;
        // 标题行：截图（由 page_title 绘制）· 计数 · 时间 · 新截图
        if !self.shots.entries.is_empty() {
            let count = format!("{} / {}", self.shots.selected + 1, self.shots.entries.len());
            c.text(52.0, 25.0, &count, size::SMALL, W::Semibold, theme::GRAY, H::Left);
        }
        let new = c.hit(300.0, 8.0, 70.0, 22.0, "shot-new");
        let (hover, press) = c.hover_t(&new);
        c.fill(300.0 + press, 8.0 + press, 70.0 - press * 2.0, 22.0 - press * 2.0, 11.0, paint::mix(theme::BTN, theme::BLUE_DEEP, 0.55 + hover * 0.45));
        c.text(335.0, 23.0, "新截图", size::SMALL, W::Semibold, theme::WHITE, H::Center);
        if new.clicked() { cmds.push(Cmd::BeginScreenshot); }

        if root.is_none() {
            self.empty_state(c, "还没有设置保存位置", "没有 E 盘：请先选择一个保存截图的文件夹", "选择文件夹", Cmd::ScreenshotChooseFolder, cmds);
        } else if self.shots.entries.is_empty() {
            if let Some(error) = self.shots.error.clone() {
                self.empty_state(c, "无法读取截图目录", &c.fit(&error, size::SMALL, W::Regular, 360.0), "重新选择", Cmd::ScreenshotChooseFolder, cmds);
            } else if self.shots.scanning || !self.shots.scanned {
                c.text(209.5, 108.0, "正在读取截图…", size::BODY, W::Regular, theme::GRAY, H::Center);
            } else {
                let hint = if self.screenshot_hotkey_label.is_empty() { "点右上角「新截图」，或使用托盘「区域截图」".to_string() } else { format!("按 {} 或点右上角「新截图」", self.screenshot_hotkey_label) };
                self.empty_state(c, "还没有截图", &hint, "开始截图", Cmd::BeginScreenshot, cmds);
            }
        } else {
            self.gallery_body(c, &env.today, cmds);
        }
    }

    fn empty_state(&mut self, c: &Card, title: &str, hint: &str, action: &str, cmd: Cmd, cmds: &mut Vec<Cmd>) {
        c.text(209.5, 92.0, title, size::BODY + 1.0, W::Semibold, theme::WHITE, H::Center);
        c.text(209.5, 112.0, hint, size::SMALL, W::Regular, theme::GRAY, H::Center);
        let response = c.hit(159.5, 128.0, 100.0, 28.0, ("shot-empty", action));
        let (hover, press) = c.hover_t(&response);
        c.fill(159.5 + press, 128.0 + press, 100.0 - press * 2.0, 28.0 - press * 2.0, 14.0, paint::mix(theme::BTN, theme::BLUE_DEEP, 0.55 + hover * 0.45));
        c.text(209.5, 147.0, action, size::SMALL, W::Semibold, theme::WHITE, H::Center);
        if response.clicked() { cmds.push(cmd); }
    }

    /// 主预览、缩略图条和操作按钮；截图页和自动预览共用。
    pub(crate) fn gallery_body(&mut self, c: &Card, today: &str, cmds: &mut Vec<Cmd>) {
        let ctx = c.ui.ctx().clone();
        // 截图页和自动预览都要请求当前项前后的解码。
        self.shots.ensure_images(&ctx);
        let Some(entry) = self.shots.current().cloned() else { return };
        // ---------- 主预览 ----------
        c.fill(MAIN.0, MAIN.1, MAIN.2, MAIN.3, 10.0, theme::CARD);
        let main = c.hit_drag(MAIN.0, MAIN.1, MAIN.2, MAIN.3, "shot-main");
        let pointer = ctx.input(|i| i.pointer.interact_pos()).map(|p| c.to_local(p));
        if main.drag_started() { if let Some(start) = pointer { self.shots.drag = Some(MainDrag { start, dx: 0.0, mode: DragMode::Undecided }); } }
        if let (Some(mut drag), Some(at)) = (self.shots.drag, pointer.filter(|_| main.dragged())) {
            let delta = at - drag.start;
            if drag.mode == DragMode::Undecided && delta.length() >= DECIDE {
                if delta.y > 0.0 && delta.y >= delta.x.abs() {
                    drag.mode = DragMode::Out;
                    cmds.push(Cmd::DragFile(entry.path.clone()));
                } else if delta.x.abs() > delta.y.abs() * 1.5 {
                    drag.mode = DragMode::Swipe;
                }
            }
            if drag.mode == DragMode::Swipe { drag.dx = delta.x; }
            self.shots.drag = Some(drag);
        }
        if main.drag_stopped() {
            if let Some(drag) = self.shots.drag.take() {
                if drag.mode == DragMode::Swipe && drag.dx.abs() >= SWIPE { self.shots.step(if drag.dx < 0.0 { 1 } else { -1 }); }
            }
        }
        if main.double_clicked() { cmds.push(Cmd::Open(entry.path.clone())); }
        let entry = self.shots.current().cloned().unwrap_or(entry);
        let swipe = self.shots.drag.filter(|drag| drag.mode == DragMode::Swipe).map_or(0.0, |drag| drag.dx * 0.35);
        let clip = c.clipped(MAIN.0, MAIN.1, MAIN.2, MAIN.3);
        match self.shots.slots.get(&entry.path) {
            Some(Slot::Ready { width, height, preview, .. }) => {
                let (x, y, w, h) = fit((*width, *height), (MAIN.0 + 4.0, MAIN.1 + 4.0, MAIN.2 - 8.0, MAIN.3 - 8.0));
                image(&clip, preview, (x + swipe, y, w, h), 6.0, Color32::WHITE);
                // 左下角：拍摄时间与原图尺寸。
                let label = format!("{} · {} × {}", when(entry.key, today), width, height);
                let lw = c.text_w(&label, size::TINY, W::Regular) + 12.0;
                c.fill(MAIN.0 + 8.0, MAIN.1 + MAIN.3 - 26.0, lw, 18.0, 9.0, Color32::from_black_alpha(165));
                c.text(MAIN.0 + 14.0, MAIN.1 + MAIN.3 - 13.0, &label, size::TINY, W::Regular, theme::WHITE, H::Left);
            }
            Some(Slot::Failed(error)) => {
                c.text(209.5, 100.0, "无法显示这张截图", size::BODY, W::Semibold, theme::GRAY, H::Center);
                c.text(209.5, 118.0, &c.fit(error, size::TINY, W::Regular, 340.0), size::TINY, W::Regular, theme::DIM, H::Center);
            }
            None => { c.text(209.5, 108.0, "正在加载…", size::SMALL, W::Regular, theme::DIM, H::Center); }
        }
        // 左右箭头只在悬停预览区时出现，到头不显示。
        let hovering = main.hovered() || main.dragged();
        for (dir, x) in [(-1isize, 34.0), (1, 385.0)] {
            let available = if dir < 0 { self.shots.selected > 0 } else { self.shots.selected + 1 < self.shots.entries.len() };
            let arrow = c.hit(x - 13.0, 90.0, 26.0, 26.0, ("shot-arrow", dir));
            let show = ctx.animate_bool_with_time(arrow.id.with("show"), available && (hovering || arrow.hovered()), 0.12);
            if show > 0.01 {
                let (hover, press) = c.hover_t(&arrow);
                paint::chevron(&c.faded(show).scaled(x, 103.0, 1.25), x, 103.0, dir as f32, hover, press);
            }
            if available && arrow.clicked() { self.shots.step(dir); }
        }

        // ---------- 缩略图条：选中项居中，前后各两张 ----------
        let (tw, th, gap) = THUMB;
        for offset in -2isize..=2 {
            let index = self.shots.selected as isize + offset;
            if index < 0 || index as usize >= self.shots.entries.len() { continue; }
            let index = index as usize;
            let path = self.shots.entries[index].path.clone();
            let x = 209.5 + offset as f32 * (tw + gap) - tw / 2.0;
            let thumb = c.hit_drag(x, THUMB_Y, tw, th, ("shot-thumb", index));
            let (hover, _) = c.hover_t(&thumb);
            c.fill(x, THUMB_Y, tw, th, 5.0, theme::CARD);
            let tint = if offset == 0 { Color32::WHITE } else { Color32::from_white_alpha((150.0 + hover * 105.0) as u8) };
            if let Some(Slot::Ready { thumb: texture, .. }) = self.shots.slots.get(&path) { image(c, texture, (x, THUMB_Y, tw, th), 5.0, tint); }
            if offset == 0 { c.stroke(x - 1.5, THUMB_Y - 1.5, tw + 3.0, th + 3.0, 6.5, 1.5, theme::WHITE); }
            if thumb.drag_started() { cmds.push(Cmd::DragFile(path)); }
            else if thumb.clicked() { self.shots.select(index); }
        }

        // ---------- 操作按钮 ----------
        let entry = self.shots.current().cloned().unwrap_or(entry);
        if self.shots.confirm_delete.as_ref() == Some(&entry.path) {
            c.text(70.0, ACTION_Y + 15.0, "将此截图移入回收站？", size::SMALL, W::Semibold, theme::WHITE, H::Left);
            if button(c, 222.0, 60.0, "取消", "cancel-delete", false, true).clicked() { self.shots.confirm_delete = None; }
            if button(c, 290.0, 90.0, "移入回收站", "confirm-delete", true, true).clicked() {
                self.shots.confirm_delete = None;
                cmds.push(Cmd::ScreenshotDelete(entry.path.clone()));
            }
        } else {
            let ready = matches!(self.shots.slots.get(&entry.path), Some(Slot::Ready { .. }));
            if button(c, 70.5, 70.0, "查看原图", "open", false, true).clicked() { cmds.push(Cmd::Open(entry.path.clone())); }
            if button(c, 148.5, 52.0, "复制", "copy", false, ready).clicked() && ready { cmds.push(Cmd::ScreenshotCopy(entry.path.clone())); }
            if button(c, 208.5, 80.0, "打开文件夹", "reveal", false, true).clicked() { cmds.push(Cmd::Reveal(entry.path.clone())); }
            if button(c, 296.5, 52.0, "删除", "delete", true, true).clicked() { self.shots.confirm_delete = Some(entry.path.clone()); }
        }
    }

    /// 方向键、滚轮、触控板横滑在截图页内切换截图，返回是否已处理（处理了就不翻页）。
    /// `cursor` 为面板局部坐标；标题行（y < 36）里的滚轮仍用来翻页。
    pub(crate) fn screenshot_input(&mut self, cursor: Option<Pos2>, wheel_y: f32, wheel_x: f32, left: bool, right: bool) -> bool {
        if left || right { self.shots.step(if right { 1 } else { -1 }); return true; }
        let Some(cursor) = cursor.filter(|p| p.y >= 36.0) else { return false };
        let _ = cursor;
        // 触控板横滑：一次手势只切一张，停顿约 0.25 秒后才接受下一次。
        if wheel_x.abs() > 0.5 {
            if self.t - self.shots.swipe_at > 0.25 { self.shots.step(if wheel_x < 0.0 { 1 } else { -1 }); }
            self.shots.swipe_at = self.t;
            return true;
        }
        if wheel_y != 0.0 {
            if self.t - self.shots.swipe_at > 0.18 { self.shots.step(if wheel_y < 0.0 { 1 } else { -1 }); }
            self.shots.swipe_at = self.t;
            return true;
        }
        true
    }

    /// 外壳报告新截图已保存。
    pub fn screenshot_saved(&mut self, ctx: &egui::Context, path: &Path, images: Arc<Images>) {
        if self.shots.root.is_none() { self.shots.rescan(ctx, store::resolve_root(&self.settings.screenshot_dir)); }
        self.shots.add_saved(ctx, path, &images);
        self.start_screenshot_preview();
    }

    /// 自动复制或「复制」按钮的结果。预览开着时显示在预览标题里，否则用提示。
    pub fn screenshot_copied(&mut self, result: Result<(), String>) {
        match self.shots.preview.as_mut() {
            Some(preview) => preview.copy = Some(result),
            None => self.say(match result { Ok(()) => "已复制图片".to_string(), Err(error) => error }),
        }
    }

    /// 删除结果：成功从列表移除，相邻的一张被选中。
    pub fn screenshot_deleted(&mut self, path: &Path, result: Result<bool, String>) {
        match result {
            Ok(true) => { self.shots.forget(path); self.say("已移入回收站"); }
            Ok(false) => self.say("已保留这张截图"),
            Err(error) => self.say(error),
        }
    }

    /// 截图后的自动预览：标题行显示保存与复制状态，其余与截图页相同。返回 true 表示已绘制。
    pub(crate) fn draw_screenshot_preview(&mut self, c: &Card, cmds: &mut Vec<Cmd>) -> bool {
        let Some(preview) = &self.shots.preview else { return false };
        let (until, copy) = (preview.until, preview.copy.clone());
        c.text(18.0, 25.0, "截图已保存", size::TITLE, W::Bold, theme::WHITE, H::Left);
        if let Some(until) = until {
            c.text(368.0, 24.0, &format!("{} 秒后收起", (until - self.t).ceil().max(0.0) as u32), size::TINY, W::Regular, theme::DIM, H::Right);
        }
        let close = c.hit(380.0, 4.0, 28.0, 28.0, "shot-preview-close");
        c.text(394.0, 23.0, "×", 18.0, W::Regular, if close.hovered() { theme::WHITE } else { theme::GRAY }, H::Center);
        if self.shots.entries.is_empty() {
            c.text(209.5, 108.0, "正在加载…", size::SMALL, W::Regular, theme::DIM, H::Center);
        } else {
            self.gallery_body(c, &self.today.clone(), cmds);
        }
        let (status, ink) = match &copy {
            None => ("正在复制…", theme::GRAY),
            Some(Ok(())) => ("已复制到剪贴板", theme::GREEN),
            Some(Err(_)) => ("未能复制，点「复制」重试", theme::RED_TOP),
        };
        let sw = c.text_w(status, size::TINY, W::Semibold) + 14.0;
        let (sx, sy) = (MAIN.0 + MAIN.2 - 8.0 - sw, MAIN.1 + MAIN.3 - 26.0);
        c.fill(sx, sy, sw, 18.0, 9.0, Color32::from_black_alpha(185));
        c.text(sx + sw / 2.0, sy + 13.0, status, size::TINY, W::Semibold, ink, H::Center);
        if close.clicked() { self.end_screenshot_preview(true); }
        true
    }

    pub(crate) fn screenshot_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let dim = color(&ctx, Color32::from_gray(180), Color32::from_gray(75));
        let warn = color(&ctx, Color32::from_rgb(255, 170, 110), Color32::from_rgb(175, 85, 20));
        ui.label(RichText::new("框选屏幕区域截图：保存为 PNG，同时复制到剪贴板。").size(12.0).color(dim));

        settings_label(ui, "显示");
        settings_card(ui, |ui| {
            let mut visible = self.settings.visible_pages[SCREENSHOT_PAGE];
            setting_row(ui, "截图页", "在岛上浏览截图历史；关闭后快捷键和托盘「区域截图」一并停用", &mut visible);
            if visible != self.settings.visible_pages[SCREENSHOT_PAGE] { self.set_page_visible(SCREENSHOT_PAGE, visible); }
        });

        settings_label(ui, "保存位置");
        settings_card(ui, |ui| {
            let root = store::resolve_root(&self.settings.screenshot_dir);
            let shown = root.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "未设置：没有 E 盘，请选择一个文件夹".into());
            ui.add(egui::Label::new(RichText::new(shown).size(13.0).color(if root.is_some() { ui.visuals().text_color() } else { warn })).truncate());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("选择文件夹…").clicked() { self.pending.push(Cmd::ScreenshotChooseFolder); }
                if let Some(root) = root.as_ref().filter(|path| path.is_dir()) {
                    if ui.button("打开文件夹").clicked() { self.pending.push(Cmd::Open(root.clone())); }
                }
                if !self.settings.screenshot_dir.is_empty() && ui.button("恢复默认").on_hover_text(store::DEFAULT_DIR).clicked() {
                    self.settings.screenshot_dir.clear();
                }
            });
            ui.label(RichText::new("截图按月份放在子文件夹里，只由你手动删除。更换位置后，原来的截图留在原处，切回即可再次看到；不会自动改存到 C 盘。").size(11.0).color(ui.visuals().weak_text_color()));
        });

        settings_label(ui, "快捷键");
        settings_card(ui, |ui| {
            let current = SCREENSHOT_HOTKEYS.iter().find(|(key, _)| *key == self.settings.screenshot_hotkey).map_or("Alt + A", |(_, label)| label);
            settings_control_row(ui, "截图", 64.0, |ui| {
                egui::ComboBox::from_id_salt("screenshot-hotkey").selected_text(current).width(160.0).show_ui(ui, |ui| {
                    for (key, label) in SCREENSHOT_HOTKEYS {
                        ui.selectable_value(&mut self.settings.screenshot_hotkey, key.to_string(), label);
                    }
                }).response
            });
            if !self.settings.visible_pages[SCREENSHOT_PAGE] {
                ui.label(RichText::new("截图页已关闭，快捷键暂不生效。").size(11.5).color(dim));
            } else if !self.screenshot_hotkey_status.is_empty() {
                let ok = !self.screenshot_hotkey_label.is_empty() || self.settings.screenshot_hotkey == "off";
                ui.label(RichText::new(&self.screenshot_hotkey_status).size(11.5).color(if ok { dim } else { warn }));
            }
            ui.label(RichText::new("快捷键对所有程序生效，会覆盖其他程序里的同名快捷键。Alt + A 也是微信的默认截图键，两个程序谁先启动谁生效。").size(11.0).color(ui.visuals().weak_text_color()));
        });
    }

    /// 外壳选好文件夹后调用；保存失败时恢复原值。
    pub fn set_screenshot_dir(&mut self, dir: PathBuf) {
        let before = self.settings.clone();
        self.settings.screenshot_dir = dir.display().to_string();
        if self.persist_settings(before) { self.say(format!("截图将保存到 {}", self.settings.screenshot_dir)); }
    }

    /// 外壳把托盘线程的注册结果转给设置页；`label` 为空表示当前没有生效的快捷键。
    pub fn set_screenshot_hotkey_status(&mut self, label: &str, message: String) {
        self.screenshot_hotkey_label = label.to_string();
        self.screenshot_hotkey_status = message;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_today_or_date() {
        let key = store::Key { stamp: 20261108_153012_328, sequence: 0 };
        assert_eq!(when(key, "2026-11-08"), "今天 15:30");
        assert_eq!(when(key, "2026-11-09"), "11月8日 15:30");
    }

    #[test]
    fn fit_keeps_aspect_and_centres() {
        let close = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| [a.0 - b.0, a.1 - b.1, a.2 - b.2, a.3 - b.3].iter().all(|d| d.abs() < 1e-3);
        assert!(close(fit((1920, 1080), (0.0, 0.0, 400.0, 100.0)), (111.111, 0.0, 177.778, 100.0)));
        assert!(close(fit((100, 1000), (10.0, 10.0, 100.0, 100.0)), (55.0, 10.0, 10.0, 100.0)));
    }
}
