//! 工资：收起时的里程表金额、展开页的跳动金币，以及设置窗口里的「工资」分组。
use crate::anim::{ease_out_cubic, smoothstep};
use crate::app::{App, Env, SALARY_PAGE};
use crate::bar_layout::{animations_enabled, SALARY_WIDTH};
use crate::extras::{setting_row, settings_card, settings_control_row, settings_label};
use crate::paint::{self, Card, H};
use crate::salary::{self, DayPhase};
use crate::settings_theme::color;
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, RichText};
use std::f32::consts::{PI, TAU};

/// 设置窗口中工资分组的编号。
pub(crate) const SETTINGS_TAB: usize = 13;
const ROLL: f64 = 0.32;
const BOUNCE: f64 = 0.5;
const FLOAT: f64 = 0.8;
const TICKER_SIZE: f32 = 11.0;
/// 收起组件中金币占用的宽度（含与金额的间距）。
const COIN_SPACE: f32 = 16.0;
const GOLD: Color32 = theme::YELLOW;
const GOLD_DEEP: Color32 = Color32::from_rgb(206, 140, 24);
const GOLD_LIGHT: Color32 = Color32::from_rgb(255, 226, 140);

struct Draft {
    monthly: String,
    config: salary::Config,
}

impl Draft {
    fn new(config: &salary::Config) -> Self {
        let monthly = match config.monthly_cents {
            0 => String::new(),
            cents if cents % 100 == 0 => (cents / 100).to_string(),
            cents => salary::format_units(cents, 2, false),
        };
        Self { monthly, config: config.clone() }
    }
}

pub(crate) struct State {
    pub(crate) config: salary::Config,
    pub(crate) snap: salary::Snapshot,
    draft: Option<Draft>,
    note: String,
    note_error: bool,
    /// 收起金额的滚动：旧文本、新文本、开始时刻。
    roll: (String, String, f64),
    /// 金币跳动的开始时刻与本次增加的金额。
    bounce: (f64, String),
    /// 上一帧的日期、小数位与金额（最小单位），用于判断是否增长。
    last: Option<(String, usize, u64)>,
    secs: Option<u32>,
    /// 当前这一秒已经过去的毫秒数，用来把刷新对齐到下一次跳秒。
    clock_ms: u16,
    /// 强制开启或关闭动画；None 时跟随 Windows「显示动画效果」设置。
    pub(crate) animations: Option<bool>,
}

impl Default for State {
    fn default() -> Self {
        Self { config: Default::default(), snap: Default::default(), draft: None, note: String::new(), note_error: false, roll: (String::new(), String::new(), f64::NEG_INFINITY), bounce: (f64::NEG_INFINITY, String::new()), last: None, secs: None, clock_ms: 0, animations: None }
    }
}

impl State {
    pub(crate) fn load() -> Self {
        Self { config: salary::Config::load(), ..Default::default() }
    }
    pub(crate) fn rolling(&self, t: f64) -> bool { t - self.roll.2 < ROLL }
    pub(crate) fn bouncing(&self, t: f64) -> bool { t - self.bounce.0 < FLOAT }
    fn text(&self) -> String { salary::format(self.snap.today, self.snap.decimals, false) }
    fn animate(&self) -> bool { self.animations.unwrap_or_else(animations_enabled) }
}

fn parse_clock(clock: &str) -> Option<u32> {
    let mut parts = clock.split(':').map(|part| part.parse::<u32>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0));
    (h < 24 && m < 60 && s < 60).then_some(h * 3600 + m * 60 + s)
}

fn duration(secs: u32) -> String {
    let minutes = secs.div_ceil(60);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}分钟"),
        (h, 0) => format!("{h}小时"),
        (h, m) => format!("{h}小时{m}分"),
    }
}

/// 月薪输入是否合法：只有数字和至多一个小数点，最多两位小数、12 个字符。
fn valid_money_input(value: &str) -> bool {
    value.len() <= 12
        && value.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && value.matches('.').count() <= 1
        && value.split_once('.').map_or(true, |(_, fraction)| fraction.len() <= 2)
}

/// 月薪输入框的缓冲区：不合法的字符在插入时直接丢弃，光标位置保持正确。
struct MoneyInput<'a>(&'a mut String);

impl egui::TextBuffer for MoneyInput<'_> {
    fn is_mutable(&self) -> bool { true }
    fn as_str(&self) -> &str { self.0 }
    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        let mut inserted = 0;
        for ch in text.chars() {
            let at = self.0.char_indices().nth(char_index + inserted).map_or(self.0.len(), |(i, _)| i);
            let mut candidate = self.0.clone();
            candidate.insert(at, ch);
            if valid_money_input(&candidate) { *self.0 = candidate; inserted += 1; }
        }
        inserted
    }
    fn delete_char_range(&mut self, char_range: std::ops::Range<usize>) {
        egui::TextBuffer::delete_char_range(self.0, char_range);
    }
}

/// 金币：`sx` 为横向缩放（翻转），`sy` 为纵向缩放（落地压扁），`vivid` 为 0 时变灰。
pub(crate) fn coin(c: &Card, x: f32, y: f32, r: f32, sx: f32, sy: f32, vivid: f32) {
    // 变灰时保留明暗层次，只去掉颜色。
    let tone = |ink: Color32| {
        let luma = (0.30 * ink.r() as f32 + 0.59 * ink.g() as f32 + 0.11 * ink.b() as f32) * 0.62;
        paint::mix(Color32::from_gray(luma as u8), ink, vivid)
    };
    let ellipse = |radius: f32, from: f32, to: f32, n: usize| -> Vec<(f32, f32)> {
        (0..=n).map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            (x + radius * sx * a.cos(), y + radius * sy * a.sin())
        }).collect()
    };
    // 填充用的多边形不能首尾重复，否则抗锯齿会在右侧留下缺口。
    let disc = |radius: f32| { let mut points = ellipse(radius, 0.0, TAU, 48); points.pop(); points };
    c.poly(&disc(r), tone(GOLD_DEEP));
    c.poly(&disc(r * 0.86), tone(GOLD));
    // 高光：左上方一段亮边。
    c.path(&ellipse(r * 0.72, PI * 1.08, PI * 1.42, 10), (r * 0.09).max(0.8), paint::alpha(tone(GOLD_LIGHT), 0.9));
    if r >= 8.0 {
        c.path(&ellipse(r * 0.7, 0.0, TAU, 48), r * 0.05, paint::alpha(tone(GOLD_DEEP), 0.55));
        let (k, ink, width) = (r * 0.3, tone(GOLD_DEEP), r * 0.1);
        let p = |dx: f32, dy: f32| (x + dx * k * sx, y + dy * k * sy);
        c.seg(p(-0.95, -1.25), p(0.0, 0.0), width, ink);
        c.seg(p(0.95, -1.25), p(0.0, 0.0), width, ink);
        c.seg(p(0.0, 0.0), p(0.0, 1.35), width, ink);
        c.seg(p(-0.8, 0.25), p(0.8, 0.25), width, ink);
        c.seg(p(-0.8, 0.85), p(0.8, 0.85), width, ink);
    } else {
        c.circle(x, y, r * 0.42, tone(GOLD_DEEP));
        c.circle(x, y, r * 0.24, tone(GOLD));
    }
}

/// 收起状态的工资组件：金币 + 金额。`roll` 为滚动前的文本与进度（0..1）；变化的数字位向上滚动。
pub(crate) fn draw_ticker(c: &Card, cx: f32, text: &str, roll: Option<(&str, f32)>, active: bool) {
    // 每位数字占固定宽度，数值变化时整体不抖动。
    let digit = (0..10).map(|d| c.text_w(&d.to_string(), TICKER_SIZE, W::Bold)).fold(0.0, f32::max);
    let dot = c.text_w(".", TICKER_SIZE, W::Bold);
    let cell = |ch: char| if ch == '.' { dot } else { digit };
    // 金币和金额作为一组在预留宽度内居中。
    let left = cx - (COIN_SPACE + text.chars().map(cell).sum::<f32>()) / 2.0;
    coin(c, left + 6.0, 15.0, 5.8, 1.0, 1.0, if active { 1.0 } else { 0.35 });
    let ink = if active { theme::WHITE } else { theme::GRAY };
    let mut x = left + COIN_SPACE;
    let from: Vec<char> = roll.map(|(from, _)| from.chars().collect()).unwrap_or_default();
    let aligned = from.len() == text.chars().count();
    let e = roll.map_or(1.0, |(_, e)| ease_out_cubic(e));
    const BASE: f32 = 19.8;
    const LIFT: f32 = 11.0;
    for (i, ch) in text.chars().enumerate() {
        let width = cell(ch);
        let center = x + width / 2.0;
        match from.get(i).copied().filter(|old| aligned && *old != ch && e < 1.0) {
            Some(old) => {
                let band = c.clipped(x - 1.0, 6.0, width + 2.0, 18.0);
                band.faded(1.0 - e).text(center, BASE - LIFT * e, &old.to_string(), TICKER_SIZE, W::Bold, ink, H::Center);
                band.faded(e).text(center, BASE + LIFT * (1.0 - e), &ch.to_string(), TICKER_SIZE, W::Bold, ink, H::Center);
            }
            None => { c.text(center, BASE, &ch.to_string(), TICKER_SIZE, W::Bold, ink, H::Center); }
        }
        x += width;
    }
}

impl App {
    /// 每帧调用一次：按当前时间重新计算，并在金额增长时触发滚动与跳动。
    pub(crate) fn salary_tick(&mut self, env: &Env) {
        let date = chrono::NaiveDate::parse_from_str(&env.today, "%Y-%m-%d").ok();
        let secs = parse_clock(&env.clock);
        let t = self.t;
        let state = &mut self.salary;
        let (Some(date), Some(secs)) = (date, secs) else {
            state.snap = Default::default();
            state.last = None;
            return;
        };
        state.secs = Some(secs);
        state.clock_ms = env.clock_ms.min(999);
        state.snap = salary::snapshot(&state.config, date, secs);
        let s = state.snap;
        let units = salary::units(s.today, s.decimals);
        if let Some((day, decimals, previous)) = &state.last {
            // 进入午休、下班的那一秒金额最后涨一次，按设计静止切换，不再滚动或跳金币。
            if *day == env.today && *decimals == s.decimals && units > *previous && s.phase == DayPhase::Working && state.animate() {
                let text = salary::format_units(units, s.decimals, false);
                let old = salary::format_units(*previous, s.decimals, false);
                if old.len() == text.len() { state.roll = (old, text, t); }
                state.bounce = (t, salary::format_units(units - previous, s.decimals, true));
            }
        }
        state.last = Some((env.today.clone(), s.decimals, units));
    }

    /// 收起时是否显示工资组件（是否放得下由 `Layout::fitted` 决定）。
    pub(crate) fn salary_on_island(&self) -> bool {
        self.settings.visible_pages[SALARY_PAGE] && self.salary.config.island
            && matches!(self.salary.snap.phase, DayPhase::Working | DayPhase::Lunch | DayPhase::AfterWork)
    }

    /// 上班中且金额可见时，到下一次跳秒还要等多久；按系统时钟的毫秒数对齐，延迟不会累积。
    pub(crate) fn salary_next_tick(&self, visible: bool) -> Option<f64> {
        (visible && self.salary.snap.phase == DayPhase::Working)
            .then(|| (1000 - self.salary.clock_ms) as f64 / 1000.0 + 0.01)
    }

    pub(crate) fn salary_animating(&self, open: bool) -> bool {
        if open { self.salary.bouncing(self.t) } else { self.salary.rolling(self.t) }
    }

    pub(crate) fn open_salary_settings(&mut self) {
        self.settings_tab = SETTINGS_TAB;
        self.open_settings();
    }

    pub(crate) fn draw_salary_bar(&mut self, c: &Card, cx: f32) {
        let state = &self.salary;
        let text = state.text();
        let roll = (state.roll.1 == text && state.rolling(self.t)).then(|| (state.roll.0.as_str(), ((self.t - state.roll.2) / ROLL) as f32));
        draw_ticker(c, cx, &text, roll, state.snap.phase == DayPhase::Working);
        if c.hit(cx - SALARY_WIDTH / 2.0 - 4.0, 1.0, SALARY_WIDTH + 8.0, 28.0, "salary-bar").clicked() {
            self.show_page(SALARY_PAGE);
        }
    }

    fn salary_status(&self) -> String {
        let config = &self.salary.config;
        let secs = self.salary.secs.unwrap_or(0);
        let left = |until: u16| duration((until as u32 * 60).saturating_sub(secs));
        match self.salary.snap.phase {
            DayPhase::Working => format!("上班中 · 距下班 {}", left(config.end)),
            DayPhase::Lunch => format!("午休中 · {} 继续", salary::format_hm(config.lunch_end)),
            DayPhase::BeforeWork => format!("{} 开工 · 还有 {}", salary::format_hm(config.start), left(config.start)),
            DayPhase::AfterWork => "已下班 · 今天辛苦了".into(),
            DayPhase::RestDay => "今天休息 · 本月累计".into(),
            DayPhase::Unconfigured => String::new(),
        }
    }

    pub(crate) fn page_salary(&mut self, c: &Card) {
        let s = self.salary.snap;
        let (cx, cy, r) = (79.0, 76.0, 24.0);
        if s.phase == DayPhase::Unconfigured {
            c.glow(cx, cy, 44.0, paint::alpha(theme::GRAY, 0.08));
            coin(c, cx, cy, r, 1.0, 1.0, 0.0);
            // 填过月薪但配置无效（例如手动改坏了 salary.txt）时，说明原因。
            let config = &self.salary.config;
            let (title, detail) = match config.validate() {
                _ if config.load_error.is_some() => ("工资配置读取失败", "已暂停计算，详情见工资设置"),
                Err(reason) if config.monthly_cents > 0 => ("工资设置需要调整", reason),
                _ => ("还没有设置工资", "填写月薪和上班时间后开始计算"),
            };
            c.text(264.0, 60.0, title, 15.0, W::Bold, theme::WHITE, H::Center);
            c.text(264.0, 80.0, detail, size::SMALL, W::Regular, theme::GRAY, H::Center);
            let button = c.hit(229.0, 92.0, 70.0, 24.0, "salary-setup");
            let (hover, press) = c.hover_t(&button);
            let b = c.scaled(264.0, 104.0, 1.0 - press * 0.04);
            b.fill(229.0, 92.0, 70.0, 24.0, 12.0, paint::mix(theme::BTN, theme::BTN_HOVER, hover));
            b.text(264.0, 108.0, "去设置", size::BODY, W::Regular, theme::WHITE, H::Center);
            if button.clicked() { self.open_salary_settings(); }
            return;
        }
        let working = s.phase == DayPhase::Working;
        let animate = self.salary.animate();
        let since = (self.t - self.salary.bounce.0) as f32;
        let e = since / BOUNCE as f32;
        let (dy, sx, sy) = if animate && (0.0..1.0).contains(&e) {
            let squash = if e > 0.85 { 1.0 - 0.08 * (PI * (e - 0.85) / 0.15).sin() } else { 1.0 };
            (-8.0 * (PI * e).sin(), 0.35 + 0.65 * (PI * e).cos().abs(), squash)
        } else { (0.0, 1.0, 1.0) };
        c.glow(cx, cy, 46.0, paint::alpha(GOLD, if working { 0.22 } else { 0.10 }));
        coin(c, cx, cy + dy, r, sx, sy, if working { 1.0 } else { 0.75 });
        let f = since / FLOAT as f32;
        if animate && (0.0..1.0).contains(&f) && !self.salary.bounce.1.is_empty() {
            let alpha = smoothstep(0.0, 0.12, f) * (1.0 - smoothstep(0.55, 1.0, f));
            // 从金币上方升起，避开左上角的页标题和跳到最高处的金币。
            c.faded(alpha).text(cx, cy - r - 5.0 - 10.0 * ease_out_cubic(f), &format!("+{}", self.salary.bounce.1), 11.0, W::Bold, GOLD, H::Center);
        }
        let rest = s.phase == DayPhase::RestDay;
        let hero = if rest { salary::format(s.month, 2, true) } else { salary::format(s.today, s.decimals, true) };
        let ink = if working { theme::WHITE } else { paint::mix(theme::WHITE, theme::GRAY, 0.6) };
        c.text(264.0, 58.0, &format!("¥{hero}"), size::HERO + 1.0, W::Bold, ink, H::Center);
        c.text(264.0, 76.0, &self.salary_status(), size::SMALL, W::Regular, theme::GRAY, H::Center);
        let money = |value: f64| salary::format(value, 2, true);
        let line = if rest { format!("月薪 ¥{} · 本月 {} 个工作日", money(s.monthly), s.workdays) }
            else { format!("本月 ¥{} / ¥{}", money(s.month), money(s.monthly)) };
        c.text(264.0, 96.0, &line, size::TINY + 0.5, W::Regular, theme::GRAY, H::Center);
        let progress = if s.monthly > 0.0 { (s.month / s.monthly).clamp(0.0, 1.0) as f32 } else { 0.0 };
        c.fill(184.0, 102.0, 160.0, 4.0, 2.0, theme::TRACK);
        if progress > 0.0 { c.fill(184.0, 102.0, (160.0 * progress).max(4.0), 4.0, 2.0, GOLD); }
        c.text(264.0, 119.0, &format!("每秒 ¥{} · 日薪 ¥{}", salary::format(s.rate, 4, false), money(s.daily)), size::TINY, W::Regular, theme::DIM, H::Center);
    }

    pub(crate) fn salary_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let dim = color(&ctx, Color32::from_gray(180), Color32::from_gray(75));
        let error_ink = color(&ctx, Color32::from_rgb(255, 130, 120), Color32::from_rgb(190, 45, 35));
        ui.label(RichText::new("按月薪与作息估算收入，数据仅保存在本机。").size(12.0).color(dim));
        settings_label(ui, "显示");
        settings_card(ui, |ui| {
            let mut visible = self.settings.visible_pages[SALARY_PAGE];
            setting_row(ui, "工资页", "在岛上显示今日工资页；关闭后收起时也不显示金额", &mut visible);
            if visible != self.settings.visible_pages[SALARY_PAGE] { self.set_page_visible(SALARY_PAGE, visible); }
        });
        if let Some(error) = &self.salary.config.load_error {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("工资配置读取失败，已禁止覆盖保存：{error}")).size(12.0).color(error_ink));
        }
        settings_label(ui, "月薪");
        let today = chrono::NaiveDate::parse_from_str(&self.today, "%Y-%m-%d").unwrap_or_else(|_| chrono::Local::now().date_naive());
        let state = &mut self.salary;
        let draft = state.draft.get_or_insert_with(|| Draft::new(&state.config));
        let previous = draft.config.clone();
        settings_card(ui, |ui| {
            settings_control_row(ui, "金额", 64.0, |ui| {
                let field = ui.add_sized([150.0, 30.0], egui::TextEdit::singleline(&mut MoneyInput(&mut draft.monthly)).vertical_align(egui::Align::Center).hint_text("例如 10000"));
                ui.label("元");
                field
            });
            draft.config.monthly_cents = salary::parse_cents(&draft.monthly).unwrap_or(0);
            ui.label(RichText::new("税前或到手均可，按你填写的金额计算。").size(11.5).color(dim));
        });
        settings_label(ui, "工作安排");
        settings_card(ui, |ui| {
            let label = 64.0;
            settings_control_row(ui, "上班日", label, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, name) in ["一", "二", "三", "四", "五", "六", "日"].into_iter().enumerate() {
                        let on = draft.config.workdays[i];
                        let button = egui::Button::new(RichText::new(name).size(12.0).color(if on { Color32::WHITE } else { color(&ctx, Color32::from_gray(170), Color32::from_gray(90)) }))
                            .fill(if on { Color32::from_rgb(53,112,218) } else { color(&ctx, Color32::from_rgb(43,49,61), Color32::from_rgb(240,244,250)) })
                            .stroke(egui::Stroke::NONE).corner_radius(6.0);
                        if ui.add_sized([30.0, 30.0], button).on_hover_text(format!("周{name}")).clicked() { draft.config.workdays[i] = !on; }
                    }
                }).response
            });
            time_range(ui, "上班时间", "salary-work", &mut draft.config.start, &mut draft.config.end);
            ui.separator();
            setting_row(ui, "午休不计薪", "午休期间金额暂停增长", &mut draft.config.lunch);
            ui.add_enabled_ui(draft.config.lunch, |ui| {
                time_range(ui, "午休时间", "salary-lunch", &mut draft.config.lunch_start, &mut draft.config.lunch_end);
            });
            ui.add_space(4.0);
            let preview = match draft.config.validate() {
                Ok(()) => {
                    let s = salary::snapshot(&draft.config, today, 0);
                    let paid = salary::paid_seconds(&draft.config, 24 * 3600).div_ceil(60);
                    let hours = if paid % 60 == 0 { format!("{} 小时", paid / 60) } else { format!("{} 小时 {} 分", paid / 60, paid % 60) };
                    format!("本月 {} 个工作日 · 每天 {hours}\n日薪 ¥{} · 每秒 ¥{}", s.workdays, salary::format(s.daily, 2, true), salary::format(s.rate, 4, false))
                }
                Err(message) => message.to_string(),
            };
            egui::Frame::new().fill(color(&ctx,Color32::from_rgb(29,44,65),Color32::from_rgb(239,245,255)))
                .corner_radius(8.0).inner_margin(10).show(ui,|ui|{
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(preview).size(12.0).color(dim));
                });
        });
        settings_label(ui, "岛上显示");
        settings_card(ui, |ui| {
            setting_row(ui, "收起时显示金额", "关闭后仅在工资页查看；开启时路过的人也能看到金额", &mut draft.config.island);
        });
        if draft.config != previous {
            state.note.clear();
            state.note_error = false;
        }
        ui.add_space(8.0);
    }

    /// 放在滚动区之外：小窗口和表单末尾都能保存，并看见校验结果。
    pub(crate) fn salary_settings_actions(&mut self, ui:&mut egui::Ui) {
        let dirty=self.salary.draft.as_ref().is_some_and(|draft|draft.config!=self.salary.config);
        let ctx=ui.ctx().clone();
        ui.separator();
        let save=ui.horizontal(|ui|{
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                let clicked=ui.add_sized([104.0,32.0],egui::Button::new(RichText::new("保存修改").color(Color32::WHITE))
                    .fill(Color32::from_rgb(53,112,218)).stroke(egui::Stroke::NONE).corner_radius(8.0)).clicked();
                ui.label(RichText::new(if dirty{"有未保存的修改"}else{"薪资与作息保存后生效"}).size(12.0)
                    .color(color(&ctx,Color32::from_rgb(169,183,205),Color32::from_rgb(101,118,143))));
                clicked
            }).inner
        }).inner;
        if save {self.save_salary();}
        if !self.salary.note.is_empty() {
            ui.label(RichText::new(&self.salary.note).size(12.0).color(if self.salary.note_error {
                color(&ctx,Color32::from_rgb(255,150,150),Color32::from_rgb(190,49,62))
            }else{color(&ctx,Color32::from_rgb(130,211,172),Color32::from_rgb(30,126,86))}));
        }
    }

    fn save_salary(&mut self) {
        let state = &mut self.salary;
        let Some(draft) = &mut state.draft else { return };
        let config = draft.config.clone();
        let result = config.validate().map_err(str::to_string).and_then(|()| config.save().map_err(|error| format!("保存失败：{error}")));
        match result {
            Ok(()) => {
                state.config = config;
                state.last = None;
                state.note = "已保存".into();
                state.note_error = false;
            }
            Err(error) => { state.note = error; state.note_error = true; }
        }
    }
}

fn time_range(ui:&mut egui::Ui,title:&str,id:&str,start:&mut u16,end:&mut u16) {
    ui.scope(|ui|{
        ui.spacing_mut().interact_size.y=30.0;
        settings_control_row(ui,title,64.0,|ui|{
            ui.spacing_mut().item_spacing.x=4.0;
            time_picker(ui,(id,"start"),start);
            ui.label("至");
            time_picker(ui,(id,"end"),end)
        });
    });
}

fn time_picker(ui: &mut egui::Ui, id: (&str,&str), minutes: &mut u16) -> egui::Response {
    let (mut hour, mut minute) = (*minutes / 60, *minutes % 60);
    let hours=egui::ComboBox::from_id_salt((id, "hour")).width(42.0).selected_text(format!("{hour:02}")).show_ui(ui, |ui| {
        for value in 0..24u16 { ui.selectable_value(&mut hour, value, format!("{value:02}")); }
    });
    ui.label(":");
    let minutes_response=egui::ComboBox::from_id_salt((id, "minute")).width(42.0).selected_text(format!("{minute:02}")).show_ui(ui, |ui| {
        for value in (0..60u16).step_by(5) { ui.selectable_value(&mut minute, value, format!("{value:02}")); }
    });
    *minutes = hour * 60 + minute;
    hours.response.union(minutes_response.response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_formats_match_the_page_copy() {
        assert_eq!(parse_clock("09:41:07"), Some(9 * 3600 + 41 * 60 + 7));
        assert_eq!(parse_clock("24:00:00"), None);
        assert_eq!(parse_clock(""), None);
        assert_eq!(duration(7 * 3600 + 30 * 60), "7小时30分");
        assert_eq!(duration(2 * 3600), "2小时");
        assert_eq!(duration(25 * 60 - 10), "25分钟");
        assert_eq!(duration(5), "1分钟");
        use egui::TextBuffer;
        let mut value = String::new();
        let mut input = MoneyInput(&mut value);
        assert_eq!(input.insert_text("1a0,000.567", 0), 8, "只插入合法字符，返回值决定光标前进几位");
        assert_eq!(input.as_str(), "10000.56");
        assert_eq!(input.insert_text(".", 2), 0, "不能再插入第二个小数点");
        assert_eq!(input.insert_text("9", 6), 0, "不能超过两位小数");
        assert_eq!(input.insert_text("9", 0), 1);
        assert_eq!(input.as_str(), "910000.56");
        input.delete_char_range(6..7);
        assert_eq!(input.as_str(), "91000056", "删掉小数点后仍然合法");
        assert_eq!(input.insert_text("１２", 0), 0, "全角数字不接受");
        assert!(valid_money_input("") && valid_money_input(".") && !valid_money_input("1234567890123"));
        assert_eq!(Draft::new(&salary::Config { monthly_cents: 1_000_050, ..Default::default() }).monthly, "10000.50");
        assert_eq!(Draft::new(&salary::Config { monthly_cents: 1_000_000, ..Default::default() }).monthly, "10000");
        assert_eq!(Draft::new(&salary::Config::default()).monthly, "");
    }

    #[test]
    fn widest_island_amount_fits_the_reserved_width() {
        let ctx = egui::Context::default();
        theme::load_fonts(&ctx);
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let c = Card::new(ui, egui::Pos2::ZERO, ui.max_rect());
                    let digit = (0..10).map(|d| c.text_w(&d.to_string(), TICKER_SIZE, W::Bold)).fold(0.0, f32::max);
                    let dot = c.text_w(".", TICKER_SIZE, W::Bold);
                    // 最宽的两种：6 位数字加小数点，或不带小数的 7 位数字（月薪很高时）。
                    let widest = ((salary::ISLAND_CHARS - 1) as f32 * digit + dot).max(salary::ISLAND_CHARS as f32 * digit);
                    assert!(COIN_SPACE + widest <= SALARY_WIDTH, "7 个字符的金额必须放进预留宽度：{widest}");
                });
            });
        }
    }

    #[test]
    fn saving_validates_before_writing_and_applies_only_on_success() {
        let _guard = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("daozai-salary-ui-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, crate::app::Options { fetch_lyrics: false, pet_idle: 1e9 });
        app.salary.draft = Some(Draft::new(&app.salary.config));
        app.save_salary();
        assert!(app.salary.note_error && app.salary.note == "请填写月薪");
        assert!(!dir.join(salary::FILE).exists(), "校验失败不能写文件");
        let draft = app.salary.draft.as_mut().unwrap();
        draft.config.monthly_cents = 1_000_000;
        draft.config.lunch_start = 7 * 60;
        app.save_salary();
        assert_eq!(app.salary.note, "午休时间要在上班时间之内");
        assert_eq!(app.salary.config, salary::Config::default(), "失败时不改变生效配置");
        app.salary.draft.as_mut().unwrap().config.lunch_start = 12 * 60;
        app.save_salary();
        assert!(!app.salary.note_error && app.salary.note == "已保存");
        assert_eq!(app.salary.config.monthly_cents, 1_000_000);
        assert_eq!(salary::Config::load(), app.salary.config);
        match previous { Some(value) => std::env::set_var("DAOZAI_DATA_DIR", value), None => std::env::remove_var("DAOZAI_DATA_DIR") }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
