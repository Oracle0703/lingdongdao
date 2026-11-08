//! 拖动岛的左右边缘调整大小，松开后记住，下次打开保持一致。
//!
//! - 收起时：改宽度。普通药丸和歌词岛分别记住；药丸不能窄于它实际的内容，歌词岛不小于 `LYRIC_WIDTH_MIN`。
//! - 展开时：整体缩放（即设置里的「岛的大小」）。各页面按固定版式绘制，只改宽度会让内容错位，所以按比例放大缩小。
//! - 双击边缘恢复默认。
use super::*;

/// 边缘感应宽度（逻辑点），左右各一条。
const EDGE: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeKind { Pill, Lyric, Panel }

#[derive(Clone)]
pub(crate) struct Resize {
    kind: ResizeKind,
    /// 左边缘 -1，右边缘 1（岛居中，两边对称变化）。
    side: f32,
    /// 按下时指针的横坐标：收起时为窗口逻辑点，展开时为屏幕物理像素（缩放会改变逻辑坐标）。
    start: f32,
    /// 按下时的宽度（逻辑点）或岛的大小（百分比）。
    value: f32,
    /// 按下时每逻辑点的物理像素数。
    ppp: f32,
    before: model::Settings,
}

impl App {
    /// 当前可以拖动的左右边缘（窗口坐标）。编辑、拖放、截图和临时预览期间不提供。
    pub(crate) fn resize_zones(&self, cx: f32, top: f32) -> Option<(ResizeKind, [Rect; 2])> {
        if self.screenshot_capturing || self.is_editing() || self.drag_hot || self.subpage.is_some() { return None; }
        let zone = |x0: f32, x1: f32, y0: f32, y1: f32| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        if self.open {
            if self.reveal < 0.95 || self.shots.preview.is_some() || self.mail.preview.is_some() { return None; }
            let (x0, x1) = (cx - PW / 2.0, cx + PW / 2.0);
            // 避开标题行和底部圆角，只取两侧的直边。
            let (y0, y1) = (top + 36.0, top + self.expanded_height() - 30.0);
            if y1 <= y0 { return None; }
            return Some((ResizeKind::Panel, [zone(x0 - 2.0, x0 + EDGE, y0, y1), zone(x1 - EDGE, x1 + 2.0, y0, y1)]));
        }
        if self.progress() > 0.02 || self.notice.is_some() && !self.compact_notices() { return None; }
        let lyric = self.island_lyrics_active();
        if self.effective_minimal() && !lyric { return None; }
        let (x0, x1) = (cx - self.w.pos / 2.0, cx + self.w.pos / 2.0);
        let kind = if lyric { ResizeKind::Lyric } else { ResizeKind::Pill };
        Some((kind, [zone(x0 - EDGE, x0 + EDGE, top, top + BAR_H), zone(x1 - EDGE, x1 + EDGE, top, top + BAR_H)]))
    }

    /// 光标是否在可拖动的边缘上（或正在拖动）。收起时据此不展开岛。
    pub(crate) fn resize_hover(&self, cx: f32, top: f32, cursor: Option<Pos2>) -> bool {
        self.resize.is_some() || self.resize_zones(cx, top).zip(cursor).is_some_and(|((_, zones), c)| zones.iter().any(|zone| zone.contains(c)))
    }

    /// 每帧调用：登记边缘的拖动区域、处理拖动和双击。放在页面绘制之后，让边缘优先于页面里的控件。
    pub(super) fn resize_input(&mut self, ui: &egui::Ui, cx: f32, top: f32, env: &Env) {
        let ctx = ui.ctx().clone();
        if let Some((kind, zones)) = self.resize_zones(cx, top).filter(|_| self.resize.is_none()) {
            for (index, zone) in zones.into_iter().enumerate() {
                let response = ui.interact(zone, egui::Id::new(("island-resize", index)), egui::Sense::click_and_drag());
                if response.hovered() { ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal); }
                if response.double_clicked() { self.reset_island_size(kind); }
                else if response.drag_started() { self.begin_resize(kind, if index == 0 { -1.0 } else { 1.0 }, &ctx, env); }
            }
        }
        if self.resize.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            if ctx.input(|i| i.pointer.primary_down()) { self.update_resize(&ctx, env, false); }
            else { self.update_resize(&ctx, env, true); self.end_resize(); }
        }
    }

    fn pointer_x(&self, kind: ResizeKind, ctx: &egui::Context, env: &Env) -> Option<f32> {
        let logical = ctx.input(|i| i.pointer.latest_pos()).map(|p| p.x);
        match kind {
            ResizeKind::Panel => env.cursor_px.map(|(x, _)| x).or(logical.map(|x| x * ctx.pixels_per_point())),
            _ => logical,
        }
    }

    fn begin_resize(&mut self, kind: ResizeKind, side: f32, ctx: &egui::Context, env: &Env) {
        let Some(now) = self.pointer_x(kind, ctx, env) else { return };
        // egui 在指针移动超过点击阈值后才报告开始拖动；从按下的位置算起，前几个像素不丢。
        let (latest, origin) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.press_origin()));
        let moved = latest.zip(origin).map_or(0.0, |(latest, origin)| latest.x - origin.x);
        let start = now - moved * if kind == ResizeKind::Panel { ctx.pixels_per_point() } else { 1.0 };
        let value = match kind {
            ResizeKind::Pill => self.base_size().0,
            ResizeKind::Lyric => self.lyric_width(),
            ResizeKind::Panel => f32::from(self.settings.ui_scale),
        };
        self.resize = Some(Resize { kind, side, start, value, ppp: ctx.pixels_per_point(), before: self.settings.clone() });
        self.hover_since = None;
        self.leave_since = None;
    }

    /// `last`：松开时按 1% 精确取值；拖动中按 5% 取整，减少重新排版字体的次数。
    fn update_resize(&mut self, ctx: &egui::Context, env: &Env, last: bool) {
        let Some(resize) = self.resize.clone() else { return };
        let Some(x) = self.pointer_x(resize.kind, ctx, env) else { return };
        let delta = (x - resize.start) * resize.side;
        match resize.kind {
            ResizeKind::Pill => {
                let natural = self.resident.target.width;
                let width = (resize.value + delta * 2.0).clamp(natural, f32::from(model::COLLAPSED_WIDTH_MAX));
                // 拖回到内容宽度就恢复自动。
                self.settings.pill_width = if width <= natural + 0.5 { 0 } else { width.round() as u16 };
                self.snap_collapsed_width();
            }
            ResizeKind::Lyric => {
                let width = (resize.value + delta * 2.0).clamp(f32::from(model::LYRIC_WIDTH_MIN), f32::from(model::COLLAPSED_WIDTH_MAX));
                self.settings.lyric_width = width.round() as u16;
                self.snap_collapsed_width();
            }
            ResizeKind::Panel => {
                let scale = resize.value * (1.0 + 2.0 * delta / (PW * resize.ppp));
                let step = if last { 1.0 } else { 5.0 };
                let (low, high) = (f32::from(*model::UI_SCALE_RANGE.start()), f32::from(*model::UI_SCALE_RANGE.end()));
                self.settings.ui_scale = ((scale / step).round() * step).clamp(low, high) as u16;
            }
        }
    }

    /// 收起宽度直接跟手，不走弹簧动画。
    fn snap_collapsed_width(&mut self) {
        let (width, _) = self.base_size();
        self.w.target = width;
        self.w.pos = width;
        self.w.vel = 0.0;
    }

    fn end_resize(&mut self) {
        let Some(resize) = self.resize.take() else { return };
        if format!("{:?}", resize.before) != format!("{:?}", self.settings) { self.persist_settings(resize.before); }
    }

    /// 双击边缘或设置里的按钮：恢复默认大小。
    pub(crate) fn reset_island_size(&mut self, kind: ResizeKind) {
        let before = self.settings.clone();
        match kind {
            ResizeKind::Pill => self.settings.pill_width = 0,
            ResizeKind::Lyric => self.settings.lyric_width = 0,
            ResizeKind::Panel => self.settings.ui_scale = 100,
        }
        if !self.open { self.snap_collapsed_width(); }
        if format!("{before:?}") != format!("{:?}", self.settings) && self.persist_settings(before) { self.say("已恢复默认大小"); }
    }
}
