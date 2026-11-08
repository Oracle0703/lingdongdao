//! 系统监控：收起时岛上的 CPU / 内存圆环，以及「系统」页（百分比、内存用量、实时上传 / 下载速率）。
//!
//! 版式（逻辑点，面板 419 × 140）：CPU 圆环 (82, 72)、内存圆环 (166, 72)，半径 25，下方 y=114 为说明；
//! 右侧 226–380 为下载 / 上传两行（44–78、82–116），每行右端是最近一分钟的走势。
use std::collections::VecDeque;

use crate::anim::smoothstep;
use crate::app::{App, SYSTEM_PAGE};
use crate::paint::{self, mix, Card, H};
use crate::perf::{self, Sample};
use crate::theme::{self, W};
use eframe::egui::{self, Color32};
use std::f32::consts::{FRAC_PI_2, TAU};

pub(crate) const CPU_INK: Color32 = Color32::from_rgb(92, 200, 255);
pub(crate) const MEMORY_INK: Color32 = Color32::from_rgb(178, 136, 255);
const DOWN_INK: Color32 = Color32::from_rgb(70, 160, 255);
const UP_INK: Color32 = Color32::from_rgb(64, 214, 130);
/// 岛上两个圆环所占的宽度。
pub(crate) const WIDTH: f32 = 46.0;
/// 走势保留的秒数。
const HISTORY: usize = 60;
/// 走势纵轴的最小量程（字节每秒），空闲时的零星流量不会被放大成满格。
const SPARK_FLOOR: f64 = 16.0 * 1024.0;

#[derive(Default)]
pub(crate) struct State {
    pub latest: Option<Sample>,
    pub history: VecDeque<Sample>,
}

impl State {
    pub(crate) fn push(&mut self, sample: Sample) {
        self.latest = Some(sample);
        self.history.push_back(sample);
        while self.history.len() > HISTORY { self.history.pop_front(); }
    }
}

/// 占用越高越偏红：80% 起渐变，95% 时为红色。
fn load_ink(base: Color32, value: f32) -> Color32 {
    mix(base, theme::RED_TOP, smoothstep(80.0, 95.0, value))
}

/// 从 12 点方向顺时针的进度环，两端圆头。
fn ring(c: &Card, x: f32, y: f32, r: f32, width: f32, value: f32, ink: Color32, track: Color32) {
    c.ring(x, y, r, width, track);
    let fraction = (value / 100.0).clamp(0.0, 1.0);
    if fraction <= 0.004 { return; }
    let (start, end) = (-FRAC_PI_2, -FRAC_PI_2 + TAU * fraction);
    c.arc(x, y, r, start, end, width, ink);
    for angle in [start, end] { c.circle(x + r * angle.cos(), y + r * angle.sin(), width / 2.0, ink); }
}

/// CPU：带引脚的芯片。
fn chip(c: &Card, x: f32, y: f32, ink: Color32) {
    c.stroke(x - 2.5, y - 2.5, 5.0, 5.0, 0.8, 1.0, ink);
    for d in [-1.2, 1.2] {
        c.seg((x + d, y - 2.5), (x + d, y - 4.0), 0.8, ink);
        c.seg((x + d, y + 2.5), (x + d, y + 4.0), 0.8, ink);
        c.seg((x - 2.5, y + d), (x - 4.0, y + d), 0.8, ink);
        c.seg((x + 2.5, y + d), (x + 4.0, y + d), 0.8, ink);
    }
}

/// 内存：带金手指的内存条。
fn memory_stick(c: &Card, x: f32, y: f32, ink: Color32) {
    c.stroke(x - 3.8, y - 2.6, 7.6, 4.0, 0.8, 1.0, ink);
    for dx in [-2.0, 0.0, 2.0] { c.seg((x + dx, y + 1.4), (x + dx, y + 3.4), 0.8, ink); }
}

/// 收起的岛上：只画 CPU 和内存两个圆环（中间是芯片和内存条图标），不显示数字。
pub(crate) fn draw_bar(c: &Card, cx: f32, cy: f32, sample: Option<&Sample>) {
    let (cpu, memory) = sample.map_or((0.0, 0.0), |s| (s.cpu, s.memory));
    let track = Color32::from_gray(52);
    for (x, value, base, is_memory) in [(cx - 12.0, cpu, CPU_INK, false), (cx + 12.0, memory, MEMORY_INK, true)] {
        let ink = load_ink(base, value);
        ring(c, x, cy, 8.5, 2.4, value, ink, track);
        let glyph = mix(base, theme::WHITE, 0.25);
        if is_memory { memory_stick(c, x, cy, glyph) } else { chip(c, x, cy, glyph) }
    }
}

/// 上 / 下箭头。
fn arrow(c: &Card, x: f32, y: f32, up: bool, ink: Color32) {
    let (tip, tail) = if up { (y - 5.0, y + 5.0) } else { (y + 5.0, y - 5.0) };
    let back = if up { tip + 3.5 } else { tip - 3.5 };
    c.seg((x, tail), (x, tip), 1.6, ink);
    c.path(&[(x - 3.5, back), (x, tip), (x + 3.5, back)], 1.6, ink);
}

/// 最近一分钟的走势：新的在右；面积淡色填充加一条折线。
fn sparkline(c: &Card, x: f32, y: f32, w: f32, h: f32, values: &[f64], ink: Color32) {
    c.seg((x, y + h), (x + w, y + h), 0.8, paint::alpha(ink, 0.22));
    if values.len() < 2 { return; }
    let max = values.iter().copied().fold(SPARK_FLOOR, f64::max);
    let offset = HISTORY - values.len();
    let points: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, value)| {
        (x + w * (offset + i) as f32 / (HISTORY - 1) as f32, y + h - h * (value / max).clamp(0.0, 1.0) as f32)
    }).collect();
    let mut mesh = egui::Mesh::default();
    let fill = paint::alpha(ink, 0.18);
    for (i, p) in points.iter().enumerate() {
        mesh.colored_vertex(c.pos(p.0, p.1), fill);
        mesh.colored_vertex(c.pos(p.0, y + h), fill);
        if i > 0 {
            let n = i as u32 * 2;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n + 1, n);
        }
    }
    c.painter.add(egui::Shape::mesh(mesh));
    c.path(&points, 1.3, ink);
}

impl App {
    /// 外壳每拿到一次采样就交给界面。
    pub fn push_perf(&mut self, sample: Sample) {
        self.perf.push(sample);
    }

    /// 是否需要后台采样：岛上显示圆环，或「系统」页开着（持续采样，打开页面就能看到最近一分钟的走势）。
    pub fn perf_wanted(&self) -> bool {
        self.settings.system_island || self.settings.visible_pages[SYSTEM_PAGE]
    }

    /// 关掉后丢弃旧数据，重新打开时不先显示过时的数值。
    pub(crate) fn tick_perf(&mut self) {
        if !self.perf_wanted() && self.perf.latest.is_some() { self.perf = State::default(); }
    }

    /// 收起时岛上实际画出了系统监控圆环（歌词、极简、通知横幅都会占用岛体）。
    pub(crate) fn system_bar_visible(&self) -> bool {
        self.resident.target.x[crate::bar_layout::SYSTEM].is_some() && self.notice.is_none()
            && !self.island_lyrics_active() && !self.effective_minimal() && !self.side()
    }

    pub(crate) fn page_system(&mut self, c: &Card) {
        let Some(sample) = self.perf.latest else {
            c.text(209.0, 80.0, "正在读取 CPU、内存和网速…", 11.0, W::Regular, theme::GRAY, H::Center);
            return;
        };
        let ctx = c.ui.ctx().clone();
        let memory = format!("内存 {}/{} GB", perf::gigabytes(sample.memory_used), perf::gigabytes(sample.memory_total));
        for (x, value, base, label, id) in [(82.0, sample.cpu, CPU_INK, "CPU".to_string(), "perf-cpu"), (166.0, sample.memory, MEMORY_INK, memory, "perf-memory")] {
            let shown = ctx.animate_value_with_time(egui::Id::new(id), value, 0.45);
            let ink = load_ink(base, shown);
            c.glow(x, 72.0, 42.0, paint::alpha(ink, 0.12));
            ring(c, x, 72.0, 25.0, 5.5, shown, ink, theme::TRACK);
            let number = format!("{:.0}", value.round());
            let (number_w, percent_w) = (c.text_w(&number, 16.0, W::Bold), c.text_w("%", 9.5, W::Semibold));
            let left = x - (number_w + 1.0 + percent_w) / 2.0;
            c.text(left, 78.0, &number, 16.0, W::Bold, theme::WHITE, H::Left);
            c.text(left + number_w + 1.0, 78.0, "%", 9.5, W::Semibold, theme::GRAY, H::Left);
            c.text(x, 114.0, &label, 10.5, W::Regular, theme::GRAY, H::Center);
        }
        let downloads: Vec<f64> = self.perf.history.iter().map(|s| s.down).collect();
        let uploads: Vec<f64> = self.perf.history.iter().map(|s| s.up).collect();
        for (row, (rate, label, up, ink, history)) in [(sample.down, "下载", false, DOWN_INK, downloads), (sample.up, "上传", true, UP_INK, uploads)].into_iter().enumerate() {
            let top = 44.0 + row as f32 * 38.0;
            arrow(c, 231.0, top + 19.0, up, ink);
            c.text(243.0, top + 11.0, label, 10.0, W::Regular, theme::GRAY, H::Left);
            let (number, unit) = perf::rate_text(rate);
            let number_w = c.text(243.0, top + 31.0, &number, 17.0, W::Bold, theme::WHITE, H::Left);
            c.text(243.0 + number_w + 3.0, top + 31.0, unit, 10.0, W::Semibold, theme::GRAY, H::Left);
            sparkline(c, 318.0, top + 8.0, 56.0, 24.0, &history, ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_last_minute() {
        let mut state = State::default();
        for i in 0..75 { state.push(Sample { cpu: i as f32, ..Default::default() }); }
        assert_eq!(state.history.len(), HISTORY);
        assert_eq!(state.history.front().unwrap().cpu, 15.0);
        assert_eq!(state.latest.unwrap().cpu, 74.0);
    }

    #[test]
    fn high_load_turns_the_ring_red() {
        assert_eq!(load_ink(CPU_INK, 50.0), CPU_INK);
        assert_eq!(load_ink(CPU_INK, 99.0), theme::RED_TOP);
    }
}
