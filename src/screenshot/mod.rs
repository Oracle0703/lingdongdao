//! 阶段 A：原生区域截图与串行 PNG 保存。正式图库将在此接口上扩展。

pub mod capture;
mod selection;

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;
use windows::Win32::Foundation::HWND;

pub const SPIKE_DIRECTORY: &str = r"E:\tmp\screenshot-spike";

#[derive(Debug)]
pub enum CaptureEvent {
    /// 遮罩、鼠标捕获和各屏冻结帧已释放；编码仍可在后台进行。
    SelectionFinished,
    Cancelled,
    Failed(String),
    Saved { path: PathBuf, width: u32, height: u32, elapsed_ms: f64 },
    SaveFailed(String),
    Copied(Result<(), String>),
}

pub(crate) struct SelectedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub confirmed_at: Instant,
}

/// 空闲时没有取帧线程；保存工作串行执行，不阻塞下一次框选。
pub struct Capture {
    events: mpsc::Receiver<CaptureEvent>,
    tx: mpsc::Sender<CaptureEvent>,
    save_tx: mpsc::Sender<Arc<SelectedImage>>,
    copy_tx: mpsc::Sender<Arc<SelectedImage>>,
    selecting: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
}

// 本仓库通过 eframe 导出 egui，避免增加独立依赖。
use eframe::egui;

impl Capture {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let (tx, events) = mpsc::channel();
        let save_tx = spawn_worker("daozai-screenshot-save", ctx.clone(), tx.clone(), save_image)?;
        // 两条串行队列共用裁剪后的像素。复制不等待磁盘，连续截图也不会乱序覆盖剪贴板。
        let copy_tx = spawn_worker("daozai-screenshot-copy", ctx.clone(), tx.clone(), copy_image)?;
        Ok(Self { events, tx, save_tx, copy_tx, selecting:Arc::new(AtomicBool::new(false)), cancel:Arc::new(AtomicBool::new(false)), ctx })
    }

    pub fn begin(&mut self, island: HWND) -> Result<(), String> {
        self.selecting.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "正在截图，请先完成或取消当前框选".to_string())?;
        self.cancel.store(false, Ordering::Release);
        let selecting = self.selecting.clone();
        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let save_tx = self.save_tx.clone();
        let copy_tx = self.copy_tx.clone();
        let ctx = self.ctx.clone();
        let island = island.0 as isize;
        if let Err(error) = std::thread::Builder::new().name("daozai-screenshot-select".into()).spawn(move || {
            let outcome = capture::select(HWND(island as *mut _), &cancel);
            // select 返回前已统一完成原生资源清理。
            let finished = match outcome {
                Ok(Some(image)) => {
                    let image = Arc::new(image);
                    if copy_tx.send(image.clone()).is_err() { let _ = tx.send(CaptureEvent::Copied(Err("截图复制线程已停止".into()))); }
                    if save_tx.send(image).is_err() { let _ = tx.send(CaptureEvent::SaveFailed("截图保存线程已停止".into())); }
                    CaptureEvent::SelectionFinished
                }
                Ok(None) => { timing("cancelled"); CaptureEvent::Cancelled }
                Err(error) => { timing(&format!("capture_failed {error}")); CaptureEvent::Failed(error) }
            };
            // 确认图片先入两条队列，再允许下一次截图；否则旧线程可能晚入队覆盖新剪贴板。
            selecting.store(false, Ordering::Release);
            let _ = tx.send(finished);
            ctx.request_repaint();
        }) {
            self.selecting.store(false, Ordering::Release);
            return Err(format!("无法启动截图线程：{error}"));
        }
        Ok(())
    }

    pub fn poll(&self) -> Vec<CaptureEvent> { self.events.try_iter().collect() }
    pub fn is_selecting(&self) -> bool { self.selecting.load(Ordering::Acquire) }
    pub fn cancel(&self) { self.cancel.store(true, Ordering::Release); }
}

impl Drop for Capture {
    // 窗口线程的定时器会结束消息循环；不在 UI 线程等待 PNG 编码。
    fn drop(&mut self) { self.cancel(); }
}

fn spawn_worker(
    name: &str,
    ctx: egui::Context,
    tx: mpsc::Sender<CaptureEvent>,
    mut work: impl FnMut(&SelectedImage) -> CaptureEvent + Send + 'static,
) -> Result<mpsc::Sender<Arc<SelectedImage>>, String> {
    let (input, queue) = mpsc::channel::<Arc<SelectedImage>>();
    std::thread::Builder::new().name(name.into()).spawn(move || {
        while let Ok(image) = queue.recv() {
            let _ = tx.send(work(&image));
            ctx.request_repaint();
        }
    }).map_err(|error| format!("无法启动截图工作线程 {name}：{error}"))?;
    Ok(input)
}

fn save_image(image: &SelectedImage) -> CaptureEvent {
    match save_png(image, Path::new(SPIKE_DIRECTORY)) {
        Ok(path) => {
            let elapsed_ms = image.confirmed_at.elapsed().as_secs_f64() * 1000.0;
            timing(&format!("saved {}x{} confirm_to_png_ms={elapsed_ms:.3} path={}", image.width, image.height, path.display()));
            CaptureEvent::Saved {path, width:image.width, height:image.height, elapsed_ms}
        }
        Err(error) => { timing(&format!("save_failed {error}")); CaptureEvent::SaveFailed(error) }
    }
}

fn copy_image(image: &SelectedImage) -> CaptureEvent {
    let result = retry_clipboard(|| {
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_image(arboard::ImageData {
            width: image.width as usize,
            height: image.height as usize,
            bytes: std::borrow::Cow::Borrowed(&image.rgba),
        })
    }, std::thread::sleep).map_err(|error| format!("无法复制截图：{error}"));
    if let Err(error) = &result { timing(&format!("copy_failed {error}")); }
    CaptureEvent::Copied(result)
}

fn retry_clipboard(
    mut operation: impl FnMut() -> Result<(), arboard::Error>,
    mut pause: impl FnMut(Duration),
) -> Result<(), arboard::Error> {
    for attempt in 0..3 {
        match operation() {
            Err(arboard::Error::ClipboardOccupied) if attempt < 2 => pause(Duration::from_millis(40 * (attempt + 1))),
            result => return result,
        }
    }
    unreachable!("最后一次尝试总会返回")
}

pub(crate) fn timing(message: &str) {
    let mode = if cfg!(debug_assertions) { "debug" } else { "release" };
    let line = format!("{} [{mode}] {message}\n", chrono::Local::now().to_rfc3339());
    eprint!("screenshot: {line}");
    let directory = Path::new(SPIKE_DIRECTORY);
    if fs::create_dir_all(directory).is_ok() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(directory.join("capture-timings.log")) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn save_png(image: &SelectedImage, directory: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|e| format!("无法创建截图目录 {}：{e}", directory.display()))?;
    let stem = chrono::Local::now().format("截图_%Y%m%d_%H%M%S_%3f").to_string();
    for sequence in 0u32.. {
        let name = if sequence == 0 { format!("{stem}.png") } else { format!("{stem}_{sequence}.png") };
        let path = directory.join(&name);
        if path.exists() { continue; }
        let part = directory.join(format!("{name}.part"));
        let file = match OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("无法写入截图 {}：{error}", part.display())),
        };
        let result = (|| -> Result<(), String> {
            let mut writer = BufWriter::new(file);
            PngEncoder::new_with_quality(&mut writer, CompressionType::Fast, FilterType::Adaptive)
                .write_image(&image.rgba, image.width, image.height, image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer.flush().map_err(|e| format!("PNG 写入失败：{e}"))?;
            writer.get_ref().sync_all().map_err(|e| format!("PNG 同步失败：{e}"))?;
            drop(writer);
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
            let from: Vec<u16> = part.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe { MoveFileExW(windows::core::PCWSTR(from.as_ptr()), windows::core::PCWSTR(to.as_ptr()), MOVEFILE_WRITE_THROUGH) }
                .map_err(|e| format!("PNG 发布失败（未覆盖已有文件）：{e}"))?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(&part); }
        result?;
        return Ok(path);
    }
    Err("截图重名序号已耗尽".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_queue_stays_ordered_and_independent_of_slow_failed_saves() {
        let (events_tx, events) = mpsc::channel();
        let (started_tx, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let ctx = egui::Context::default();
        let save = spawn_worker("test-screenshot-save", ctx.clone(), events_tx.clone(), move |_| {
            started_tx.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(3)).unwrap();
            CaptureEvent::SaveFailed("合成写盘失败".into())
        }).unwrap();
        let copy = spawn_worker("test-screenshot-copy", ctx, events_tx, |image| {
            assert_eq!((image.width, image.height, image.rgba.len()), (2, 2, 16));
            CaptureEvent::Copied(if image.rgba[0] == 1 { Err("第一张合成复制失败".into()) } else { Ok(()) })
        }).unwrap();
        let image = |value| Arc::new(SelectedImage {width:2,height:2,rgba:vec![value;16],confirmed_at:Instant::now()});
        let first = image(1);
        save.send(first.clone()).unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        copy.send(first).unwrap();
        copy.send(image(2)).unwrap();
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::Copied(Err(error)) if error=="第一张合成复制失败"));
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::Copied(Ok(()))), "上一张失败不能阻止下一张复制，且不等待慢写盘");
        release.send(()).unwrap();
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::SaveFailed(_)));
    }

    #[test]
    fn clipboard_busy_retries_are_bounded_and_other_errors_do_not_retry() {
        let mut calls = 0;
        let mut delays = Vec::new();
        retry_clipboard(|| {
            calls += 1;
            if calls < 3 { Err(arboard::Error::ClipboardOccupied) } else { Ok(()) }
        }, |delay| delays.push(delay)).unwrap();
        assert_eq!(calls, 3);
        assert_eq!(delays, vec![Duration::from_millis(40), Duration::from_millis(80)]);
        calls = 0;
        let result = retry_clipboard(|| { calls += 1; Err(arboard::Error::ClipboardOccupied) }, |_| {});
        assert!(matches!(result, Err(arboard::Error::ClipboardOccupied)));
        assert_eq!(calls, 3);
        calls = 0;
        let result = retry_clipboard(|| { calls += 1; Err(arboard::Error::ConversionFailure) }, |_| panic!("非占用错误不应等待重试"));
        assert!(matches!(result, Err(arboard::Error::ConversionFailure)));
        assert_eq!(calls, 1);
    }

    /// 合成图片编码/写盘基准；不是确认保存到真实截图完成的端到端测量。
    #[test]
    #[ignore = "显式 release 基准，仅写 E:\\tmp\\screenshot-spike\\bench 的合成 PNG"]
    fn stage_a_release_png_benchmark() {
        assert!(!cfg!(debug_assertions), "请使用 cargo test --release 运行性能测试");
        let directory = Path::new(SPIKE_DIRECTORY).join("bench");
        for (width, height) in [(400u32,300u32),(3840,2160)] {
            let mut rgba = Vec::with_capacity((width*height*4) as usize);
            for y in 0..height {
                for x in 0..width { rgba.extend_from_slice(&[(x%256) as u8,(y%256) as u8,((x/8)^(y/8)) as u8,255]); }
            }
            let mut image = SelectedImage {width,height,rgba,confirmed_at:Instant::now()};
            for sample in 1..=5 {
                image.confirmed_at = Instant::now();
                let path = save_png(&image,&directory).unwrap();
                let elapsed_ms = image.confirmed_at.elapsed().as_secs_f64()*1000.0;
                assert_eq!(image::image_dimensions(&path).unwrap(),(width,height));
                timing(&format!("bench_png synthetic sample={sample} size={width}x{height} encode_write_ms={elapsed_ms:.3} path={}",path.display()));
            }
        }
    }
}
