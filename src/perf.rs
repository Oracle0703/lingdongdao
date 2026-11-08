//! 本机 CPU、内存占用与网络上传 / 下载速率的采样。
//!
//! 后台线程每秒读一次系统计数器（GetSystemTimes、GlobalMemoryStatusEx、GetIfTable2），
//! 只在需要时运行；结果经通道交给界面，界面在下一次正常刷新时显示，不额外唤醒窗口。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 一次采样的结果。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// CPU 占用（0..100）。
    pub cpu: f32,
    /// 内存占用（0..100）。
    pub memory: f32,
    /// 已用 / 总物理内存（字节）。
    pub memory_used: u64,
    pub memory_total: u64,
    /// 下载 / 上传速率（字节每秒）。
    pub down: f64,
    pub up: f64,
}

/// 系统空闲、内核（含空闲）与用户态时间的累计值（100 纳秒）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// 两次读数之间的 CPU 占用百分比。内核时间包含空闲时间，所以忙碌 = 内核 + 用户 − 空闲。
pub fn cpu_usage(previous: CpuTimes, now: CpuTimes) -> f32 {
    let total = now.kernel.saturating_sub(previous.kernel) + now.user.saturating_sub(previous.user);
    if total == 0 { return 0.0; }
    let idle = now.idle.saturating_sub(previous.idle).min(total);
    ((total - idle) as f64 / total as f64 * 100.0) as f32
}

/// 每个网卡（按 LUID）累计收发的字节数。
pub type Counters = HashMap<u64, (u64, u64)>;

/// 两次读数之间的下载、上传速率（字节每秒）。只比较两次都在的网卡，
/// 网卡新出现、消失或计数器归零时不会算出虚高的速率。
pub fn net_rates(previous: &Counters, now: &Counters, seconds: f64) -> (f64, f64) {
    if seconds <= 0.0 { return (0.0, 0.0); }
    let (mut down, mut up) = (0u64, 0u64);
    for (luid, (received, sent)) in now {
        if let Some((old_received, old_sent)) = previous.get(luid) {
            down += received.saturating_sub(*old_received);
            up += sent.saturating_sub(*old_sent);
        }
    }
    (down as f64 / seconds, up as f64 / seconds)
}

/// 速率的显示文字（数值, 单位）：不到 1 MB/s 用 KB/s，达到后换成 MB/s（1 KB = 1024 字节）。
pub fn rate_text(bytes_per_second: f64) -> (String, &'static str) {
    let kb = bytes_per_second.max(0.0) / 1024.0;
    // 按显示精度判断进位，避免出现「1024 KB/s」或「10.0」这类边界写法。
    if kb < 1023.5 {
        if kb < 9.95 { (format!("{kb:.1}"), "KB/s") } else { (format!("{kb:.0}"), "KB/s") }
    } else {
        let mb = kb / 1024.0;
        let text = if mb < 9.995 { format!("{mb:.2}") } else if mb < 99.95 { format!("{mb:.1}") } else { format!("{mb:.0}") };
        (text, "MB/s")
    }
}

/// 内存大小（GB，1 GB = 1024³ 字节，保留一位小数）。
pub fn gigabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// 启动采样线程。`enabled` 为 false 时线程休眠，重新打开后先取一次基准，一秒后才有第一份结果。
pub fn monitor(enabled: Arc<AtomicBool>) -> Receiver<Sample> {
    let (tx, rx) = mpsc::sync_channel(4);
    std::thread::Builder::new().name("daozai-perf".into()).spawn(move || {
        let mut baseline: Option<(Instant, CpuTimes, Counters)> = None;
        loop {
            if !enabled.load(Ordering::Relaxed) {
                baseline = None;
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let now = Instant::now();
            let (Some(times), counters) = (read_cpu(), read_counters()) else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            if let Some((at, previous_times, previous_counters)) = &baseline {
                let (down, up) = net_rates(previous_counters, &counters, now.duration_since(*at).as_secs_f64());
                let (memory_used, memory_total) = read_memory().unwrap_or_default();
                let memory = if memory_total > 0 { (memory_used as f64 / memory_total as f64 * 100.0) as f32 } else { 0.0 };
                let sample = Sample { cpu: cpu_usage(*previous_times, times), memory, memory_used, memory_total, down, up };
                match tx.try_send(sample) {
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                    _ => {}
                }
            }
            baseline = Some((now, times, counters));
            std::thread::sleep(Duration::from_secs(1).saturating_sub(now.elapsed()));
        }
    }).ok();
    rx
}

#[cfg(windows)]
fn read_cpu() -> Option<CpuTimes> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    let value = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some(CpuTimes { idle: value(idle), kernel: value(kernel), user: value(user) })
}

#[cfg(windows)]
fn read_memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullTotalPhys.saturating_sub(status.ullAvailPhys), status.ullTotalPhys))
}

/// 只统计已连接的物理网卡：VPN、虚拟网卡和筛选驱动的流量最终也经过物理网卡，一起算会重复。
#[cfg(windows)]
fn read_counters() -> Counters {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    const HARDWARE: u8 = 1;
    const FILTER: u8 = 2;
    const LOOPBACK: u32 = 24;
    let mut counters = Counters::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    unsafe {
        if GetIfTable2(&mut table).is_err() || table.is_null() { return counters; }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        for row in rows {
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            if flags & HARDWARE == 0 || flags & FILTER != 0 || row.Type == LOOPBACK || row.OperStatus != IfOperStatusUp { continue; }
            counters.insert(row.InterfaceLuid.Value, (row.InOctets, row.OutOctets));
        }
        FreeMibTable(table as *const _);
    }
    counters
}

#[cfg(not(windows))]
fn read_cpu() -> Option<CpuTimes> { None }
#[cfg(not(windows))]
fn read_memory() -> Option<(u64, u64)> { None }
#[cfg(not(windows))]
fn read_counters() -> Counters { Counters::new() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_counts_kernel_and_user_time_minus_idle() {
        let before = CpuTimes { idle: 1_000, kernel: 2_000, user: 500 };
        let after = CpuTimes { idle: 1_600, kernel: 2_800, user: 700 };
        // 总计 800 + 200 = 1000，其中空闲 600：占用 40%。
        assert!((cpu_usage(before, after) - 40.0).abs() < 1e-4);
        assert_eq!(cpu_usage(before, before), 0.0, "没有时间流逝时为 0");
        let odd = CpuTimes { idle: 5_000, kernel: 2_100, user: 500 };
        assert_eq!(cpu_usage(before, odd), 0.0, "空闲多于总时间（计数异常）时不出现负数");
    }

    #[test]
    fn net_rates_only_compare_adapters_seen_twice() {
        let previous = Counters::from([(1, (1_000, 500)), (2, (10, 10))]);
        let now = Counters::from([(1, (3_048, 1_524)), (3, (9_999_999, 9_999_999)), (2, (0, 0))]);
        let (down, up) = net_rates(&previous, &now, 2.0);
        assert_eq!((down, up), (1_024.0, 512.0), "新网卡不算，计数器归零的网卡不算负数");
        assert_eq!(net_rates(&previous, &now, 0.0), (0.0, 0.0));
    }

    #[test]
    fn rates_switch_from_kb_to_mb_at_one_megabyte() {
        assert_eq!(rate_text(0.0), ("0.0".into(), "KB/s"));
        assert_eq!(rate_text(512.0), ("0.5".into(), "KB/s"));
        assert_eq!(rate_text(12.3 * 1024.0), ("12".into(), "KB/s"));
        assert_eq!(rate_text(1023.0 * 1024.0), ("1023".into(), "KB/s"));
        assert_eq!(rate_text(1023.6 * 1024.0), ("1.00".into(), "MB/s"), "四舍五入到 1024 KB 时直接换成 MB");
        assert_eq!(rate_text(1.25 * 1024.0 * 1024.0), ("1.25".into(), "MB/s"));
        assert_eq!(rate_text(56.78 * 1024.0 * 1024.0), ("56.8".into(), "MB/s"));
        assert_eq!(rate_text(320.0 * 1024.0 * 1024.0), ("320".into(), "MB/s"));
        assert_eq!(rate_text(-5.0), ("0.0".into(), "KB/s"));
    }

    #[test]
    fn memory_is_shown_in_gigabytes() {
        assert_eq!(gigabytes(16 * 1024 * 1024 * 1024), "16.0");
        assert_eq!(gigabytes(10_522_669_875), "9.8");
    }

    #[cfg(windows)]
    #[test]
    fn this_machine_reports_plausible_counters() {
        let times = read_cpu().expect("读取 CPU 时间");
        assert!(times.kernel >= times.idle);
        let (used, total) = read_memory().expect("读取内存");
        assert!(total > 0 && used <= total);
        let _ = read_counters();
    }

    /// 在本机实际跑几秒采样线程，打印结果供人工核对：`cargo test monitor_reports_real_samples -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn monitor_reports_real_samples() {
        let enabled = Arc::new(AtomicBool::new(true));
        let rx = monitor(enabled.clone());
        for _ in 0..4 {
            let sample = rx.recv_timeout(Duration::from_secs(3)).expect("每秒一份采样");
            let (down, down_unit) = rate_text(sample.down);
            let (up, up_unit) = rate_text(sample.up);
            println!("CPU {:.1}%  内存 {:.1}% ({}/{} GB)  下载 {down} {down_unit}  上传 {up} {up_unit}", sample.cpu, sample.memory, gigabytes(sample.memory_used), gigabytes(sample.memory_total));
            assert!((0.0..=100.0).contains(&sample.cpu) && (0.0..=100.0).contains(&sample.memory));
        }
        enabled.store(false, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(1200));
        while rx.try_recv().is_ok() {}
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "关掉后不再采样");
    }
}
