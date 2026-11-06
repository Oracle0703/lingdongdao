//! 工资实时计数：按月薪、上班日与作息算出今天和本月已经赚到的钱。
//!
//! 纯逻辑，不依赖界面。金额每次都由「配置 + 当前时间」直接算出，不累加、不保存进度，
//! 所以重启或睡眠唤醒后数字依然正确。配置存放在 `config_dir()/salary.txt`。
use chrono::{Datelike, NaiveDate};

pub(crate) const FILE: &str = "salary.txt";
/// 月薪上限：1000 万元（单位：分）。
pub(crate) const MAX_CENTS: u64 = 1_000_000_000;
/// 收起状态最多显示的字符数（含小数点）。
pub(crate) const ISLAND_CHARS: usize = 7;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// 月薪（分）；0 表示未配置。
    pub monthly_cents: u64,
    /// 周一到周日是否上班。
    pub workdays: [bool; 7],
    /// 上下班时间（当天第几分钟）。
    pub start: u16,
    pub end: u16,
    /// 午休不计薪；关闭时保留时间，方便再次打开。
    pub lunch: bool,
    pub lunch_start: u16,
    pub lunch_end: u16,
    /// 收起时在岛上显示金额。
    pub island: bool,
    pub load_error: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self { monthly_cents: 0, workdays: [true, true, true, true, true, false, false], start: 9 * 60, end: 18 * 60, lunch: true, lunch_start: 12 * 60, lunch_end: 13 * 60, island: true, load_error: None }
    }
}

pub(crate) fn parse_hm(value: &str) -> Option<u16> {
    let (h, m) = value.trim().split_once(':')?;
    let (h, m) = (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?);
    (h < 24 && m < 60).then(|| h * 60 + m)
}

pub(crate) fn format_hm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// 「10000」「10000.5」「.5」→ 分；空白、非数字或超过两位小数时返回 None。
pub(crate) fn parse_cents(value: &str) -> Option<u64> {
    let value = value.trim();
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 2 { return None; }
    if !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) { return None; }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let fraction: u64 = format!("{fraction:0<2}").parse().ok()?;
    whole.checked_mul(100)?.checked_add(fraction)
}

impl Config {
    fn path() -> std::path::PathBuf {
        crate::model::config_dir().join(FILE)
    }

    pub fn load() -> Self {
        let mut config = Self::default();
        let values = match crate::model::read_kv_checked(&Self::path()) {
            Ok(values) => values,
            Err(error) => { config.load_error = Some(error); return config; }
        };
        let flag = |value: &str| match value { "0" => Some(false), "1" => Some(true), _ => None };
        for (key, value) in values {
            // 缺少的字段用默认值；写了但看不懂的字段不猜，整份配置视为读取失败（停止计算、禁止覆盖保存）。
            let ok = match key.as_str() {
                "monthly_cents" => value.parse().ok().filter(|v| *v <= MAX_CENTS).map(|v| config.monthly_cents = v).is_some(),
                "workdays" => (value.len() == 7 && value.bytes().all(|b| b == b'0' || b == b'1'))
                    .then(|| for (day, on) in config.workdays.iter_mut().zip(value.bytes()) { *day = on == b'1'; })
                    .is_some(),
                "start" => parse_hm(&value).map(|m| config.start = m).is_some(),
                "end" => parse_hm(&value).map(|m| config.end = m).is_some(),
                "lunch" => flag(&value).map(|on| config.lunch = on).is_some(),
                "lunch_start" => parse_hm(&value).map(|m| config.lunch_start = m).is_some(),
                "lunch_end" => parse_hm(&value).map(|m| config.lunch_end = m).is_some(),
                "island" => flag(&value).map(|on| config.island = on).is_some(),
                _ => true,
            };
            if !ok && config.load_error.is_none() {
                config.load_error = Some(format!("{FILE} 中 {key}={value} 无法识别"));
            }
        }
        config
    }

    pub fn save(&self) -> std::io::Result<()> {
        crate::model::ensure_loaded(&self.load_error)?;
        crate::model::write_kv(&Self::path(), &[
            ("monthly_cents".into(), self.monthly_cents.to_string()),
            ("workdays".into(), self.workdays.iter().map(|on| if *on { '1' } else { '0' }).collect()),
            ("start".into(), format_hm(self.start)),
            ("end".into(), format_hm(self.end)),
            ("lunch".into(), (self.lunch as u8).to_string()),
            ("lunch_start".into(), format_hm(self.lunch_start)),
            ("lunch_end".into(), format_hm(self.lunch_end)),
            ("island".into(), (self.island as u8).to_string()),
        ])
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.monthly_cents == 0 { return Err("请填写月薪"); }
        if self.monthly_cents > MAX_CENTS { return Err("月薪不能超过 1000 万元"); }
        if !self.workdays.contains(&true) { return Err("请至少选择一个上班日"); }
        if self.start >= self.end || self.end >= 24 * 60 { return Err("下班时间要晚于上班时间"); }
        if self.lunch {
            if !(self.start <= self.lunch_start && self.lunch_start < self.lunch_end && self.lunch_end <= self.end) {
                return Err("午休时间要在上班时间之内");
            }
            if self.lunch_end - self.lunch_start >= self.end - self.start { return Err("除去午休后没有上班时间"); }
        }
        Ok(())
    }

    pub(crate) fn is_workday(&self, date: NaiveDate) -> bool {
        self.workdays[date.weekday().num_days_from_monday() as usize]
    }
}

/// 当天截至 `secs`（当天第几秒）的计薪秒数，不含午休。
pub fn paid_seconds(config: &Config, secs: u32) -> u32 {
    let (start, end) = (config.start as u32 * 60, config.end as u32 * 60);
    let worked = secs.min(end).saturating_sub(start);
    let lunch = if config.lunch { secs.min(config.lunch_end as u32 * 60).saturating_sub(config.lunch_start as u32 * 60) } else { 0 };
    worked.saturating_sub(lunch)
}

/// 本月中，日期在 `before` 号之前（不含）且星期几被勾选的天数；`before` 为 32 时即整月。
fn workdays_until(workdays: &[bool; 7], year: i32, month: u32, before: u32) -> u32 {
    (1..before)
        .filter_map(|day| NaiveDate::from_ymd_opt(year, month, day))
        .filter(|date| workdays[date.weekday().num_days_from_monday() as usize])
        .count() as u32
}

pub fn workdays_in_month(workdays: &[bool; 7], year: i32, month: u32) -> u32 {
    workdays_until(workdays, year, month, 32)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DayPhase {
    #[default]
    Unconfigured,
    RestDay,
    BeforeWork,
    Working,
    Lunch,
    AfterWork,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub phase: DayPhase,
    /// 元/秒。
    pub rate: f64,
    pub today: f64,
    pub daily: f64,
    pub month: f64,
    pub monthly: f64,
    pub workdays: u32,
    pub decimals: usize,
}

pub fn snapshot(config: &Config, date: NaiveDate, secs: u32) -> Snapshot {
    if config.load_error.is_some() || config.validate().is_err() { return Snapshot::default(); }
    let workdays = workdays_in_month(&config.workdays, date.year(), date.month());
    let daily_secs = paid_seconds(config, 24 * 3600);
    let monthly = config.monthly_cents as f64 / 100.0;
    let rate = monthly / (workdays as f64 * daily_secs as f64);
    let workday = config.is_workday(date);
    let paid = if workday { paid_seconds(config, secs) } else { 0 };
    let before = workdays_until(&config.workdays, date.year(), date.month(), date.day());
    let (start, end) = (config.start as u32 * 60, config.end as u32 * 60);
    let lunch = config.lunch && (config.lunch_start as u32 * 60..config.lunch_end as u32 * 60).contains(&secs);
    let phase = if !workday { DayPhase::RestDay }
        else if secs < start { DayPhase::BeforeWork }
        else if secs >= end { DayPhase::AfterWork }
        else if lunch { DayPhase::Lunch }
        else { DayPhase::Working };
    let daily = rate * daily_secs as f64;
    Snapshot {
        phase, rate, today: rate * paid as f64, daily,
        month: rate * (before as f64 * daily_secs as f64 + paid as f64),
        monthly, workdays, decimals: decimals(rate, daily),
    }
}

/// 小数位：保证最后一位大约每秒都会变化；按日薪确定，当天不变，收起状态不超过 7 个字符。
pub fn decimals(rate: f64, daily: f64) -> usize {
    let mut decimals = if rate >= 0.01 { 2 } else if rate >= 0.001 { 3 } else { 4 };
    while decimals > 0 && format(daily, decimals, false).chars().count() > ISLAND_CHARS { decimals -= 1; }
    decimals
}

/// 按小数位向下取整后的最小单位数，不会多显示还没赚到的钱。
pub fn units(value: f64, decimals: usize) -> u64 {
    // 余量吸收浮点误差，避免 9999.999999 被显示成 9999.99。
    (value.max(0.0) * 10f64.powi(decimals as i32) + 1e-6).floor() as u64
}

pub fn format_units(units: u64, decimals: usize, grouping: bool) -> String {
    let scale = 10u64.pow(decimals as u32);
    let whole = (units / scale).to_string();
    let whole = if grouping {
        let mut out = String::new();
        for (i, ch) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i) % 3 == 0 { out.push(','); }
            out.push(ch);
        }
        out
    } else { whole };
    if decimals == 0 { whole } else { format!("{whole}.{:0width$}", units % scale, width = decimals) }
}

pub fn format(value: f64, decimals: usize, grouping: bool) -> String {
    format_units(units(value, decimals), decimals, grouping)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config { Config { monthly_cents: 1_000_000, ..Default::default() } }
    fn day(y: i32, m: u32, d: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, d).unwrap() }
    fn at(h: u32, m: u32) -> u32 { h * 3600 + m * 60 }

    #[test]
    fn sample_values_match_the_design_document() {
        let config = sample();
        assert_eq!(workdays_in_month(&config.workdays, 2026, 11), 21);
        assert_eq!(workdays_in_month(&config.workdays, 2026, 2), 20);
        assert_eq!(workdays_in_month(&config.workdays, 2026, 12), 23);
        let six = [true, true, true, true, true, true, false];
        assert_eq!(workdays_in_month(&six, 2026, 11), 25);
        assert_eq!(paid_seconds(&config, 24 * 3600), 28800);
        let s = snapshot(&config, day(2026, 11, 5), at(10, 30));
        assert!((s.rate - 0.016534391).abs() < 1e-8);
        assert_eq!(s.decimals, 2);
        assert_eq!(format(s.daily, 2, false), "476.19");
        assert_eq!(format(s.today, 2, false), "89.28");
        assert_eq!(format(s.month, 2, true), "1,517.85");
        assert_eq!(s.phase, DayPhase::Working);
        let lunch = snapshot(&config, day(2026, 11, 5), at(12, 30));
        assert_eq!(lunch.phase, DayPhase::Lunch);
        assert_eq!(format(lunch.today, 2, false), "178.57");
        assert_eq!(lunch.today, snapshot(&config, day(2026, 11, 5), at(12, 0)).today);
        let low = snapshot(&Config { monthly_cents: 300_000, ..sample() }, day(2026, 11, 5), at(10, 30));
        assert!((low.rate - 0.00496).abs() < 1e-5);
        assert_eq!(low.decimals, 3);
    }

    #[test]
    fn paid_seconds_pause_for_lunch_and_stop_after_work() {
        let config = sample();
        assert_eq!(paid_seconds(&config, at(8, 0)), 0);
        assert_eq!(paid_seconds(&config, at(9, 0)), 0);
        assert_eq!(paid_seconds(&config, at(9, 0) + 1), 1);
        assert_eq!(paid_seconds(&config, at(12, 0)), 3 * 3600);
        assert_eq!(paid_seconds(&config, at(12, 59)), 3 * 3600);
        assert_eq!(paid_seconds(&config, at(13, 30)), 3 * 3600 + 1800);
        assert_eq!(paid_seconds(&config, at(18, 0)), 8 * 3600);
        assert_eq!(paid_seconds(&config, at(23, 59)), 8 * 3600);
        assert_eq!(paid_seconds(&Config { lunch: false, ..sample() }, at(23, 0)), 9 * 3600);
    }

    #[test]
    fn last_workday_of_the_month_earns_exactly_the_salary() {
        for cents in [1_000_000, 300_000, 1_234_567, 99_999_999] {
            let config = Config { monthly_cents: cents, ..sample() };
            let s = snapshot(&config, day(2026, 11, 30), at(18, 0));
            assert_eq!(units(s.month, 2), cents, "月底累计应正好等于月薪");
            assert_eq!(snapshot(&config, day(2026, 11, 30), at(23, 59)).month, s.month);
        }
    }

    #[test]
    fn day_phases_follow_the_schedule() {
        let config = sample();
        let phase = |date, secs| snapshot(&config, date, secs).phase;
        let thursday = day(2026, 11, 5);
        assert_eq!(phase(thursday, at(8, 59)), DayPhase::BeforeWork);
        assert_eq!(phase(thursday, at(9, 0)), DayPhase::Working);
        assert_eq!(phase(thursday, at(12, 0)), DayPhase::Lunch);
        assert_eq!(phase(thursday, at(13, 0)), DayPhase::Working);
        assert_eq!(phase(thursday, at(18, 0)), DayPhase::AfterWork);
        let saturday = day(2026, 11, 7);
        assert_eq!(phase(saturday, at(10, 0)), DayPhase::RestDay);
        assert_eq!(snapshot(&config, saturday, at(10, 0)).today, 0.0);
        // 周六的本月累计等于周一到周五五天的日薪。
        assert_eq!(format(snapshot(&config, saturday, at(10, 0)).month, 2, false), "2380.95");
        // 跨天：周五下班后到周六，今日归零，本月累计保留。
        let friday = snapshot(&config, day(2026, 11, 6), at(23, 59));
        assert_eq!(friday.phase, DayPhase::AfterWork);
        assert_eq!(units(friday.month, 2), units(snapshot(&config, saturday, 0).month, 2));
        assert_eq!(phase(thursday, at(10, 0)), DayPhase::Working);
        assert_eq!(snapshot(&Config::default(), thursday, at(10, 0)).phase, DayPhase::Unconfigured);
    }

    #[test]
    fn validation_explains_each_problem() {
        assert_eq!(Config::default().validate(), Err("请填写月薪"));
        assert_eq!(Config { monthly_cents: MAX_CENTS + 1, ..sample() }.validate(), Err("月薪不能超过 1000 万元"));
        assert_eq!(Config { workdays: [false; 7], ..sample() }.validate(), Err("请至少选择一个上班日"));
        assert_eq!(Config { start: 18 * 60, end: 9 * 60, ..sample() }.validate(), Err("下班时间要晚于上班时间"));
        assert_eq!(Config { lunch_start: 8 * 60, ..sample() }.validate(), Err("午休时间要在上班时间之内"));
        assert_eq!(Config { lunch_start: 9 * 60, lunch_end: 18 * 60, ..sample() }.validate(), Err("除去午休后没有上班时间"));
        assert_eq!(Config { lunch: false, lunch_start: 8 * 60, ..sample() }.validate(), Ok(()));
        assert_eq!(sample().validate(), Ok(()));
    }

    #[test]
    fn formatting_floors_groups_and_limits_island_width() {
        assert_eq!(format(476.190476, 2, false), "476.19");
        assert_eq!(format(0.999, 2, false), "0.99");
        assert_eq!(format(1234567.891, 2, true), "1,234,567.89");
        assert_eq!(format(999.0, 0, true), "999");
        assert_eq!(format(12.5, 4, false), "12.5000");
        assert_eq!(decimals(0.02, 476.19), 2);
        assert_eq!(decimals(0.005, 142.85), 3);
        assert_eq!(decimals(0.0005, 14.4), 4);
        assert_eq!(decimals(0.5, 14285.71), 1, "超过 7 个字符时减少小数");
        assert_eq!(decimals(5.0, 142857.14), 0);
        for (rate, daily) in [(0.016, 476.19), (0.00496, 142.857), (0.0004, 11.52), (0.08, 2380.95), (0.5, 14285.7)] {
            assert!(format(daily, decimals(rate, daily), false).chars().count() <= ISLAND_CHARS);
        }
    }

    #[test]
    fn parse_cents_accepts_money_and_rejects_noise() {
        assert_eq!(parse_cents("10000"), Some(1_000_000));
        assert_eq!(parse_cents(" 10000.5 "), Some(1_000_050));
        assert_eq!(parse_cents("10000.55"), Some(1_000_055));
        assert_eq!(parse_cents("10000."), Some(1_000_000));
        assert_eq!(parse_cents(".5"), Some(50));
        for bad in ["", ".", "abc", "1.234", "1,000", "-5", "1.2.3"] { assert_eq!(parse_cents(bad), None, "{bad}"); }
    }

    #[test]
    fn config_round_trips_and_refuses_to_overwrite_unreadable_files() {
        let _guard = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("daozai-salary-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        assert_eq!(Config::load(), Config::default());
        let config = Config { monthly_cents: 1_234_567, workdays: [true, false, true, false, true, true, false], start: 8 * 60 + 30, end: 17 * 60 + 45, lunch: false, lunch_start: 11 * 60 + 55, lunch_end: 12 * 60 + 40, island: false, load_error: None };
        config.save().unwrap();
        assert_eq!(Config::load(), config);
        // 缺少的字段用默认值，不认识的键忽略（留给以后的新版本）。
        std::fs::write(dir.join(FILE), "monthly_cents=1000000\nfuture_key=whatever\n").unwrap();
        assert_eq!(Config::load(), Config { monthly_cents: 1_000_000, ..Config::default() });
        // 写了但看不懂的字段：不猜默认值，停止计算并拒绝覆盖保存。
        let valid = "monthly_cents=1000000\nworkdays=1111100\nstart=09:00\nend=18:00\nlunch=1\nlunch_start=12:00\nlunch_end=13:00\nisland=0\n";
        std::fs::write(dir.join(FILE), valid).unwrap();
        assert!(Config::load().load_error.is_none());
        let date = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        for (old, bad) in [("monthly_cents=1000000", "monthly_cents=abc"), ("monthly_cents=1000000", "monthly_cents=1000000001"), ("workdays=1111100", "workdays=xx"),
            ("start=09:00", "start=25:00"), ("end=18:00", "end=1093:00"), ("lunch=1", "lunch=yes"), ("lunch_start=12:00", "lunch_start=1:65535"),
            ("lunch_end=13:00", "lunch_end=65535:00"), ("island=0", "island=false")] {
            let input = valid.replace(old, bad);
            std::fs::write(dir.join(FILE), &input).unwrap();
            let broken = Config::load();
            assert_eq!(broken.load_error.as_deref(), Some(format!("{FILE} 中 {bad} 无法识别").as_str()));
            assert_eq!(snapshot(&broken, date, 10 * 3600 + 30 * 60).phase, DayPhase::Unconfigured, "{bad} 时不计薪");
            assert!(broken.save().is_err(), "{bad} 时不能覆盖原文件");
            assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), input);
        }
        std::fs::remove_file(dir.join(FILE)).unwrap();
        std::fs::create_dir(dir.join(FILE)).unwrap();
        let unreadable = Config::load();
        assert!(unreadable.load_error.is_some());
        assert!(Config { monthly_cents: 1, ..unreadable }.save().is_err(), "读取失败时不能覆盖原文件");
        match previous { Some(value) => std::env::set_var("DAOZAI_DATA_DIR", value), None => std::env::remove_var("DAOZAI_DATA_DIR") }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
