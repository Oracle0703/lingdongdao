//! 沙漏：岛上的「沙漏」页（翻转开始的倒计时），以及收起时岛上的小沙漏。
//!
//! 版式（逻辑点，面板 419 × 140）：左侧沙漏 52–116 × 36–122，点一下翻转；右侧剩余时间、
//! 时长预设和开始 / 暂停。没在计时的沙漏，沙子都在下半边；翻转后沙子在上半边，开始往下流。
//! 计时中再翻转，和真沙漏一样上下两边的沙子对调（剩余时间变成已过去的时间）。
use crate::anim::{self, smoothstep};
use crate::app::{App, HOURGLASS_PAGE};
use crate::paint::{self, capsule, mix, Card, H};
use crate::extras::PetState;
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32};
use std::f32::consts::{PI, TAU};

/// 沙子与页面强调色。
pub(crate) const SAND: Color32 = Color32::from_rgb(236, 190, 116);
const FRAME: Color32 = Color32::from_rgb(178, 132, 84);
/// 翻转动画时长（秒）。翻转期间不响应再次点击，双击不会把刚翻过来的沙漏又翻回去。
const FLIP_SECS: f64 = 0.6;
/// 页面上的时长预设（分钟）。
pub(crate) const PRESETS: [u32; 5] = [1, 3, 5, 10, 15];
pub(crate) const MINUTES_MAX: u32 = 120;
/// 时间到之后「时间到了」提示保留的秒数。
const DONE_HINT_SECS: f64 = 60.0;

// ---------- 玻璃形状（单位坐标：中心为原点，上下各 1，向下为正） ----------
const NECK: f32 = 0.06;
/// 玻璃最宽处的半宽 / 半高。
const BULB_W: f32 = 0.6;
/// 沙子最多堆到的位置（留出底部描边）。
const FILL_TO: f32 = 0.97;
const STEPS: usize = 48;

pub(crate) struct State {
    /// 一轮的总时长（秒），来自设置里的沙漏时长。
    pub total: f32,
    /// 上半边还剩的沙子（秒）。
    pub remaining: f32,
    pub running: bool,
    /// 已经翻转、沙子分在上下两边；没开始或已流完时为 false（沙子都在下半边）。
    pub active: bool,
    flip_at: f64,
    /// 翻转前上半边的沙量（0..1），翻转动画期间按它画。
    flip_top: f32,
    /// 上一次流完的时间。
    pub done_at: f64,
    /// 点重置的次数，驱动重置图标转一圈。
    resets: u32,
}

impl State {
    pub(crate) fn new(minutes: u32) -> Self {
        let total = minutes.clamp(1, MINUTES_MAX) as f32 * 60.0;
        Self { total, remaining: total, running: false, active: false, flip_at: f64::NEG_INFINITY, flip_top: 0.0, done_at: f64::NEG_INFINITY, resets: 0 }
    }
    /// 上半边的沙量（0..1）；下半边是 1 减去它。
    pub(crate) fn top(&self) -> f32 {
        if self.active { (self.remaining / self.total).clamp(0.0, 1.0) } else { 0.0 }
    }
    pub(crate) fn flipping(&self, now: f64) -> bool {
        (0.0..FLIP_SECS).contains(&(now - self.flip_at))
    }
    /// 翻转角度（弧度，0..π，顺时针）；不在翻转时为 0。
    fn angle(&self, now: f64) -> f32 {
        if !self.flipping(now) { return 0.0; }
        PI * anim::ease_in_out(((now - self.flip_at) / FLIP_SECS) as f32)
    }
    /// 画面上的上半边沙量：翻转动画期间是翻转前的状态（随玻璃一起转过去）。
    fn shown_top(&self, now: f64) -> f32 {
        if self.flipping(now) { self.flip_top } else { self.top() }
    }
    /// 翻转沙漏。没开始时从头开始流；计时中上下两边的沙子对调。翻转动画期间忽略，返回 false。
    pub(crate) fn flip(&mut self, now: f64) -> bool {
        if self.flipping(now) { return false; }
        self.flip_top = self.top();
        self.flip_at = now;
        if self.active {
            self.remaining = self.total - self.remaining;
            // 刚开始就翻回去：沙子几乎都回到下半边，当作没开始，不提示时间到。
            if self.remaining < 1.0 { self.stop(); return true; }
        } else {
            self.remaining = self.total;
            self.active = true;
        }
        self.running = true;
        true
    }
    /// 停止并回到没开始的状态（沙子都在下半边）。
    pub(crate) fn stop(&mut self) {
        self.remaining = self.total;
        self.running = false;
        self.active = false;
    }
    pub(crate) fn set_minutes(&mut self, minutes: u32) {
        self.total = minutes.clamp(1, MINUTES_MAX) as f32 * 60.0;
        self.stop();
    }
    /// 推进时间；沙子刚好流完时返回 true。
    pub(crate) fn advance(&mut self, elapsed: f32, now: f64) -> bool {
        if !self.running { return false; }
        self.remaining -= elapsed;
        if self.remaining > 0.0 { return false; }
        self.stop();
        self.done_at = now;
        true
    }
}

fn mmss(secs: f32) -> String {
    let secs = secs.max(0.0).ceil() as u32;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// 玻璃在 |v| = a 处的半宽（相对 `BULB_W`）：瓶颈最细，约 0.8 处最宽，两端略收。
fn profile(a: f32) -> f32 {
    NECK + (1.0 - NECK) * (a.clamp(0.0, 1.0) * 0.62 * PI).sin()
}

/// 从瓶颈到 |v| = a 的累计截面积（按宽度积分，近似沙子的体积）。
fn areas() -> [f32; STEPS + 1] {
    let mut cumulative = [0.0; STEPS + 1];
    let step = FILL_TO / STEPS as f32;
    for i in 1..=STEPS {
        let (a0, a1) = ((i - 1) as f32 * step, i as f32 * step);
        cumulative[i] = cumulative[i - 1] + (profile(a0) + profile(a1)) / 2.0 * step;
    }
    cumulative
}

/// 累计面积为 `area` 时的位置 |v|。
fn level(cumulative: &[f32; STEPS + 1], area: f32) -> f32 {
    let area = area.clamp(0.0, cumulative[STEPS]);
    let i = cumulative.partition_point(|value| *value < area).clamp(1, STEPS);
    let (low, high) = (cumulative[i - 1], cumulative[i]);
    let f = if high > low { (area - low) / (high - low) } else { 0.0 };
    (i as f32 - 1.0 + f) * FILL_TO / STEPS as f32
}

/// 上半边的沙子（单位坐标）：沙面中间略凹，沿玻璃壁收到瓶颈。返回轮廓和用于扇形填充的内部点。
fn top_sand(fraction: f32) -> Option<(Vec<(f32, f32)>, (f32, f32))> {
    if fraction <= 0.002 { return None; }
    let cumulative = areas();
    let a = level(&cumulative, fraction * cumulative[STEPS]);
    let dip = (0.07 * smoothstep(0.0, 0.25, fraction)).min(a * 0.4);
    let half = BULB_W * profile(a);
    let mut points: Vec<(f32, f32)> = (0..=10).map(|i| {
        let s = -1.0 + i as f32 / 5.0;
        (s * half, -a + dip * (1.0 - s * s))
    }).collect();
    points.extend((0..8).rev().map(|j| { let a = a * j as f32 / 8.0; (BULB_W * profile(a), -a) }));
    points.extend((0..8).map(|j| { let a = a * j as f32 / 8.0; (-BULB_W * profile(a), -a) }));
    Some((points, (0.0, -a * 0.5)))
}

/// 下半边的沙子：从底部堆起，沙面中间隆起。
fn bottom_sand(fraction: f32) -> Option<(Vec<(f32, f32)>, (f32, f32), f32)> {
    if fraction <= 0.002 { return None; }
    let cumulative = areas();
    let a = level(&cumulative, (1.0 - fraction) * cumulative[STEPS]);
    let mound = (0.1 * smoothstep(0.0, 0.2, fraction)).min((a - 0.04).max(0.0));
    let half = BULB_W * profile(a);
    let mut points: Vec<(f32, f32)> = (0..=10).map(|i| {
        let s = -1.0 + i as f32 / 5.0;
        (s * half, a - mound * (1.0 - smoothstep(0.0, 1.0, s.abs())))
    }).collect();
    points.extend((1..=8).map(|j| { let a = a + (FILL_TO - a) * j as f32 / 8.0; (BULB_W * profile(a), a) }));
    points.extend((1..=8).rev().map(|j| { let a = a + (FILL_TO - a) * j as f32 / 8.0; (-BULB_W * profile(a), a) }));
    Some((points, (0.0, (a + FILL_TO) * 0.5), a - mound))
}

/// 以 `center` 为扇心填充（形状需从扇心可见全部边界），再沿轮廓描一圈同色细线做抗锯齿。
fn fan(c: &Card, center: (f32, f32), points: &[(f32, f32)], ink: Color32, edge: f32) {
    if points.len() < 3 { return; }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c.pos(center.0, center.1), ink);
    for p in points { mesh.colored_vertex(c.pos(p.0, p.1), ink); }
    let n = points.len() as u32;
    for i in 0..n { mesh.add_triangle(0, i + 1, (i + 1) % n + 1); }
    c.painter.add(egui::Shape::mesh(mesh));
    if edge > 0.0 {
        let mut outline = points.to_vec();
        outline.push(points[0]);
        c.path(&outline, edge, ink);
    }
}

/// 画一个沙漏。`size` 是半高（逻辑点）；`top` 是上半边沙量；`angle` 为翻转角度；
/// `stream` 为 Some(时间) 时画流下的沙线和沙粒；`detailed` 为 true 时画木架和玻璃高光（页面用）。
fn glass(c: &Card, x: f32, y: f32, size: f32, top: f32, angle: f32, stream: Option<f32>, detailed: bool) {
    let (sin, cos) = angle.sin_cos();
    let at = |(u, v): (f32, f32)| (x + (u * cos - v * sin) * size, y + (u * sin + v * cos) * size);
    let map = |points: &[(f32, f32)]| points.iter().map(|p| at(*p)).collect::<Vec<_>>();
    let line = (size * 0.036).max(1.0);
    let frame_w = BULB_W * 1.2;
    if detailed {
        for side in [-1.0, 1.0] {
            let u = side * BULB_W * 1.1;
            c.seg(at((u, -1.0)), at((u, 1.0)), size * 0.05, mix(FRAME, Color32::BLACK, 0.18));
        }
    }
    // 玻璃：从瓶颈看出去整个轮廓都可见，所以可以以中心扇形填充。
    let outline_side = |side: f32| (0..=40).map(|k| { let v = -1.0 + k as f32 / 20.0; (side * BULB_W * profile(v.abs()), v) }).collect::<Vec<_>>();
    let (right, left) = (outline_side(1.0), outline_side(-1.0));
    let mut body = right.clone();
    body.extend(left.iter().rev());
    fan(c, at((0.0, 0.0)), &map(&body), Color32::from_white_alpha(12), 0.0);
    let sand = SAND;
    if let Some((points, center)) = top_sand(top) {
        fan(c, at(center), &map(&points), sand, line * 0.5);
    }
    let bottom = bottom_sand(1.0 - top);
    if let Some((points, center, _)) = &bottom {
        fan(c, at(*center), &map(points), mix(sand, Color32::BLACK, 0.06), line * 0.5);
    }
    if let (Some(time), true) = (stream, top > 0.002) {
        let end = bottom.as_ref().map_or(FILL_TO, |(_, _, peak)| *peak);
        let width = (size * 0.032).max(0.8) * (0.5 + 0.5 * smoothstep(0.0, 0.06, top));
        c.seg(at((0.0, 0.0)), at((0.0, end)), width, sand);
        for i in 0..3 {
            let p = (time * 1.8 + i as f32 / 3.0).fract();
            let (gx, gy) = at(((i as f32 - 1.0) * 0.012, 0.04 + (end - 0.04) * p * p));
            c.circle(gx, gy, width * 0.9, mix(sand, theme::WHITE, 0.35));
        }
    }
    let glass_ink = Color32::from_white_alpha(150);
    c.path(&map(&right), line, glass_ink);
    c.path(&map(&left), line, glass_ink);
    if detailed {
        let glint = |sign: f32| (0..=8).map(|k| { let a = 0.42 + 0.36 * k as f32 / 8.0; (-BULB_W * profile(a) * 0.72, sign * a) }).collect::<Vec<_>>();
        c.path(&map(&glint(-1.0)), line, Color32::from_white_alpha(70));
        c.path(&map(&glint(1.0)), line * 0.8, Color32::from_white_alpha(40));
    }
    let cap = if detailed { 0.09 } else { 0.16 };
    for (v0, v1) in [(-1.0 - cap, -1.0), (1.0, 1.0 + cap)] {
        c.poly(&map(&[(-frame_w, v0), (frame_w, v0), (frame_w, v1), (-frame_w, v1)]), FRAME);
    }
}

/// 收起的岛上：小沙漏 + 剩余时间，与番茄钟共用同一个位置和宽度。
pub(crate) fn draw_bar(c: &Card, cx: f32, cy: f32, t: f64, state: &State, compact: bool) {
    let width = if compact { crate::focus_bar::LYRIC_WIDTH } else { crate::focus_bar::WIDTH };
    let left = cx - width / 2.0;
    let flowing = state.running && crate::bar_layout::animations_enabled();
    glass(c, left + 12.0, cy, 10.5, state.top(), 0.0, state.running.then_some(if flowing { t as f32 } else { 0.0 }), false);
    c.text(left + width - 20.0, cy + 4.8, &mmss(state.remaining), if compact { 10.0 } else { 11.0 }, W::Bold, SAND, H::Center);
}

/// 提醒卡片上的图标：流完的沙漏。
pub(crate) fn alert_icon(c: &Card, x: f32, y: f32) {
    glass(c, x, y, 11.0, 0.0, 0.0, None, false);
}

impl App {
    /// 收起时岛上的计时组件显示沙漏还是番茄钟：两个都在计时时显示先结束的那个。
    pub(crate) fn hourglass_on_island(&self) -> bool {
        self.hourglass.running && (!self.pomo.running || self.hourglass.remaining < self.pomo.remaining)
    }

    /// 每帧推进：跟随设置里的时长；沙子流完时提醒。
    pub(crate) fn tick_hourglass(&mut self, elapsed: f32) {
        let minutes = self.settings.hourglass_minutes;
        if !self.hourglass.active && self.hourglass.total != minutes as f32 * 60.0 { self.hourglass.set_minutes(minutes); }
        if self.hourglass.advance(elapsed, self.t) {
            self.pet.react(PetState::Happy);
            let minutes = (self.hourglass.total / 60.0).round() as u32;
            self.alert("沙漏时间到", format!("{minutes} 分钟的沙子流完了"), HOURGLASS_PAGE, true);
        }
    }

    fn set_hourglass_minutes(&mut self, minutes: u32) {
        let minutes = minutes.clamp(1, MINUTES_MAX);
        let before = self.settings.clone();
        self.settings.hourglass_minutes = minutes;
        if before.hourglass_minutes == minutes || self.persist_settings(before) { self.hourglass.set_minutes(minutes); }
    }

    pub(crate) fn page_hourglass(&mut self, c: &Card) {
        let now = self.t;
        let ctx = c.ui.ctx().clone();
        let (gx, gy) = (84.0, 79.0);
        let active = self.hourglass.active;
        let run = ctx.animate_bool_with_time(egui::Id::new("hourglass-run"), self.hourglass.running, 0.3);
        let idle = 1.0 - ctx.animate_bool_with_time(egui::Id::new("hourglass-active"), active, 0.2);
        let breathe = 0.5 + 0.5 * ((now * 2.0).sin() as f32);
        c.glow(gx, gy, 54.0, paint::alpha(SAND, 0.08 + 0.12 * run * breathe));
        // 系统关闭了动画效果时：不转动、沙粒不动，直接显示翻转后的状态。
        let animate = crate::bar_layout::animations_enabled();
        let flipping = animate && self.hourglass.flipping(now);
        let stream = (self.hourglass.running && !flipping).then(|| if animate { now as f32 } else { 0.0 });
        let hit = c.hit(gx - 32.0, 36.0, 64.0, 86.0, "hourglass-flip");
        let (hover, press) = c.hover_t(&hit);
        let k = 1.0 + 0.03 * hover - 0.04 * press;
        let (top, angle) = if flipping { (self.hourglass.shown_top(now), self.hourglass.angle(now)) } else { (self.hourglass.top(), 0.0) };
        glass(&c.scaled(gx, gy, k), gx, gy, 38.0, top, angle, stream, true);
        let hit = hit.on_hover_text(if active { "翻转沙漏：上下两边的沙子对调" } else { "翻转沙漏，开始计时" });
        if hit.clicked() { self.hourglass.flip(now); }

        let minutes = self.settings.hourglass_minutes;
        let shown = if active { self.hourglass.remaining } else { self.hourglass.total };
        c.text(264.0, 61.0, &mmss(shown), size::HERO + 1.0, W::Bold, if active { theme::WHITE } else { mix(theme::WHITE, SAND, 0.25) }, H::Center);
        let status = if self.hourglass.running {
            format!("共 {} 分钟", (self.hourglass.total / 60.0).round() as u32)
        } else if active {
            "已暂停".to_string()
        } else if now - self.hourglass.done_at < DONE_HINT_SECS {
            "时间到了，点沙漏再来一次".to_string()
        } else {
            "点沙漏翻转开始".to_string()
        };
        c.text(264.0, 79.0, &status, size::SMALL, W::Regular, theme::GRAY, H::Center);

        // 没开始时：时间两侧的 − / + 按分钟微调；计时中换成重置。
        if idle > 0.02 {
            let b = c.faded(idle);
            for (x, label, delta, id) in [(204.0, "−", -1i32, "hourglass-minus"), (324.0, "+", 1, "hourglass-plus")] {
                let enabled = if delta < 0 { minutes > 1 } else { minutes < MINUTES_MAX };
                let response = b.hit(x - 11.0, 40.0, 22.0, 22.0, id);
                let (hover, press) = b.hover_t(&response);
                let button = b.scaled(x, 51.0, 1.0 - 0.1 * press);
                button.circle(x, 51.0, 11.0, mix(theme::BTN, theme::BTN_HOVER, hover));
                button.centered_text(x, 51.0, label, 13.0, W::Semibold, if enabled { theme::WHITE } else { theme::DIM }, H::Center);
                let response = response.on_hover_text(if delta < 0 { "少 1 分钟" } else { "多 1 分钟" });
                if !active && enabled && response.clicked() { self.set_hourglass_minutes(minutes.saturating_add_signed(delta)); }
            }
        }
        if idle < 0.98 {
            let b = c.faded(1.0 - idle);
            let response = b.hit(317.0, 38.0, 24.0, 24.0, "hourglass-reset");
            let (hover, press) = b.hover_t(&response);
            let button = b.scaled(329.0, 50.0, 1.0 - 0.1 * press);
            button.circle(329.0, 50.0, 11.0, mix(Color32::from_rgb(28, 28, 30), Color32::from_rgb(50, 50, 54), hover));
            let spin = ctx.animate_value_with_time(egui::Id::new("hourglass-reset-spin"), self.hourglass.resets as f32 * TAU, 0.45);
            paint::reset(&button, 329.0, 50.0, -spin, Color32::from_gray(230));
            if active && response.on_hover_text("停止并重置").clicked() {
                self.hourglass.stop();
                self.hourglass.resets += 1;
            }
        }

        // 时长预设 + 开始 / 暂停。
        let selected_minutes = if active { (self.hourglass.total / 60.0).round() as u32 } else { minutes };
        for (index, preset) in PRESETS.into_iter().enumerate() {
            let x = 147.0 + index as f32 * 39.0;
            let selected = selected_minutes == preset;
            let background = if selected { mix(theme::BTN, SAND, 0.24) } else { theme::BTN };
            let (response, button) = capsule(c, x, 94.0, 34.0, 25.0, &format!("hourglass-preset-{preset}"), background, if selected { mix(theme::BTN_HOVER, SAND, 0.28) } else { theme::BTN_HOVER });
            button.text(x + 17.0, 110.0, &format!("{preset}分"), size::BODY, W::Semibold, if selected { mix(SAND, theme::WHITE, 0.45) } else { theme::WHITE }, H::Center);
            let response = response.on_hover_text(if active { format!("改成 {preset} 分钟（会停止当前计时）") } else { format!("{preset} 分钟") });
            if response.clicked() { self.set_hourglass_minutes(preset); }
        }
        let (response, button) = capsule(c, 345.0, 94.0, 36.0, 25.0, "hourglass-start", Color32::from_rgb(176, 124, 58), Color32::from_rgb(204, 150, 78));
        paint::play_pause(&button, 363.0, 106.5, 11.0, run, theme::WHITE);
        let response = response.on_hover_text(if self.hourglass.running { "暂停" } else if active { "继续" } else { "开始" });
        if response.clicked() {
            if active { self.hourglass.running = !self.hourglass.running; } else { self.hourglass.flip(now); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flipping_starts_then_swaps_sand_like_a_real_hourglass() {
        let mut glass = State::new(3);
        assert_eq!(glass.top(), 0.0, "没开始时沙子都在下半边");
        assert!(glass.flip(0.0));
        assert!(glass.running && glass.active);
        assert_eq!(glass.remaining, 180.0);
        assert!(!glass.flip(0.3), "翻转动画期间不再响应，双击不会翻回去");
        assert!(!glass.advance(60.0, 60.0));
        assert_eq!(glass.remaining, 120.0);
        assert!(glass.flip(60.0));
        assert_eq!(glass.remaining, 60.0, "上下对调：剩余时间变成已过去的时间");
        assert!(glass.advance(60.0, 120.0), "流完时报告一次");
        assert!(!glass.running && !glass.active);
        assert_eq!(glass.remaining, 180.0, "流完后回到没开始，下一轮仍是完整时长");
        assert!(!glass.advance(1.0, 121.0));
    }

    #[test]
    fn flipping_back_right_after_starting_stops_without_finishing() {
        let mut glass = State::new(1);
        glass.flip(0.0);
        glass.advance(0.7, 0.7);
        assert!(glass.flip(1.0));
        assert!(!glass.active && !glass.running, "沙子几乎都回到下半边：当作没开始");
        assert_eq!(glass.done_at, f64::NEG_INFINITY, "不算流完，不提醒");
    }

    #[test]
    fn minutes_are_clamped_and_changing_them_stops() {
        let mut glass = State::new(0);
        assert_eq!(glass.total, 60.0);
        glass.flip(0.0);
        glass.set_minutes(500);
        assert_eq!(glass.total, MINUTES_MAX as f32 * 60.0);
        assert!(!glass.running && !glass.active);
    }

    #[test]
    fn flip_turns_half_a_circle_and_settles_upright() {
        let mut glass = State::new(1);
        glass.flip(10.0);
        assert_eq!(glass.angle(10.0), 0.0);
        assert!((glass.angle(10.3) - PI / 2.0).abs() < 0.01);
        assert_eq!(glass.angle(10.0 + FLIP_SECS + 0.01), 0.0, "转完回到正立，画面按新的沙量画");
        assert_eq!(glass.shown_top(10.2), 0.0, "转动中按翻转前的沙量画");
        assert_eq!(glass.shown_top(11.0), 1.0);
    }

    #[test]
    fn sand_stays_inside_the_glass_and_grows_with_its_share() {
        let inside = |(u, v): (f32, f32)| v.abs() <= FILL_TO + 1e-3 && u.abs() <= BULB_W * profile(v.abs()) + 1e-3;
        let area = |points: &[(f32, f32)]| points.iter().zip(points.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<f32>().abs() / 2.0;
        assert!(top_sand(0.0).is_none() && bottom_sand(0.0).is_none());
        let mut previous = (0.0, 0.0);
        for fraction in [0.01, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0] {
            let (top, _) = top_sand(fraction).unwrap();
            let (bottom, _, peak) = bottom_sand(fraction).unwrap();
            assert!(top.iter().all(|p| inside(*p) && p.1 <= 1e-3), "上半边的沙子不越过玻璃和瓶颈");
            assert!(bottom.iter().all(|p| inside(*p) && p.1 >= 0.0), "下半边的沙子不越过玻璃和瓶颈");
            assert!((0.0..=FILL_TO).contains(&peak), "沙堆顶不越过瓶颈");
            let now = (area(&top), area(&bottom));
            assert!(now.0 > previous.0 && now.1 > previous.1, "沙量越多面积越大");
            previous = now;
        }
        let (full, _) = top_sand(1.0).unwrap();
        let reach = full.iter().map(|p| -p.1).fold(0.0, f32::max);
        assert!(reach > 0.9, "满沙时上半边几乎装满");
    }

    #[test]
    fn countdown_text_rounds_up() {
        assert_eq!(mmss(180.0), "03:00");
        assert_eq!(mmss(0.2), "00:01");
        assert_eq!(mmss(-1.0), "00:00");
    }
}
