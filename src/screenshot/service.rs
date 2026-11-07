//! 截图服务：每次截图一个框选线程；确认后的图片进入两条串行队列——保存到截图目录、复制到剪贴板。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use super::{capture, store};

#[derive(Debug)]
pub enum CaptureEvent {
    /// 遮罩、鼠标捕获和各屏冻结帧已释放；编码仍可在后台进行。
    SelectionFinished,
    Cancelled,
    Failed(String),
    /// `images`：直接由内存像素生成的预览图和缩略图，界面不必再从磁盘解码。
    Saved { path: PathBuf, width: u32, height: u32, elapsed_ms: f64, images: Arc<super::library::Images> },
    /// 保存失败：图片留在内存里（见 `Capture::retry_save` 等），`pending` 为目前未保存的张数。
    SaveFailed { error: String, images: Arc<super::library::Images>, pending: usize },
    Copied(Result<(), String>),
}

/// 未能保存、仍留在内存里的截图张数；托盘「退出」据此提醒。
pub static UNSAVED: AtomicUsize = AtomicUsize::new(0);

type Failed = Arc<Mutex<Vec<Arc<SelectedImage>>>>;

pub(crate) struct SelectedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub confirmed_at: Instant,
    /// 发起截图时的保存目录；`None` 表示还没有可用的目录。之后改设置不影响已排队的图片。
    pub root: Option<PathBuf>,
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
    failed: Failed,
}

// 本仓库通过 eframe 导出 egui，避免增加独立依赖。
use eframe::egui;

impl Capture {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let (tx, events) = mpsc::channel();
        let failed: Failed = Arc::default();
        let worker_failed = failed.clone();
        let save_tx = spawn_worker("daozai-screenshot-save", ctx.clone(), tx.clone(), move |image| save_image(image, &worker_failed))?;
        // 两条串行队列共用裁剪后的像素。复制不等待磁盘，连续截图也不会乱序覆盖剪贴板。
        let copy_tx = spawn_worker("daozai-screenshot-copy", ctx.clone(), tx.clone(), copy_image)?;
        Ok(Self { events, tx, save_tx, copy_tx, selecting:Arc::new(AtomicBool::new(false)), cancel:Arc::new(AtomicBool::new(false)), ctx, failed })
    }

    pub fn begin(&mut self, island: HWND, root: Option<PathBuf>) -> Result<(), String> {
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
                Ok(Some(mut image)) => {
                    image.root = root;
                    let image = Arc::new(image);
                    if copy_tx.send(image.clone()).is_err() { let _ = tx.send(CaptureEvent::Copied(Err("截图复制线程已停止".into()))); }
                    if save_tx.send(image).is_err() { let _ = tx.send(CaptureEvent::Failed("截图保存线程已停止，截图没有保存".into())); }
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

    /// 图库里的「复制」：后台解码文件，再走同一条剪贴板队列，结果仍是 `Copied`。
    pub fn copy_path(&self, path: PathBuf) {
        let (copy_tx, tx, ctx) = (self.copy_tx.clone(), self.tx.clone(), self.ctx.clone());
        let _ = std::thread::Builder::new().name("daozai-screenshot-copy-file".into()).spawn(move || {
            match image::open(&path) {
                Ok(decoded) => {
                    let decoded = decoded.to_rgba8();
                    let (width, height) = decoded.dimensions();
                    let image = SelectedImage { width, height, rgba: decoded.into_raw(), confirmed_at: Instant::now(), root: None };
                    if copy_tx.send(Arc::new(image)).is_err() { let _ = tx.send(CaptureEvent::Copied(Err("截图复制线程已停止".into()))); }
                }
                Err(error) => { let _ = tx.send(CaptureEvent::Copied(Err(format!("无法读取这张截图：{error}")))); }
            }
            ctx.request_repaint();
        });
    }

    /// 是否有保存失败、还留在内存里的截图。有的话不开始新截图，先让用户处理。
    pub fn has_failed(&self) -> bool { !self.failed.lock().unwrap_or_else(|e| e.into_inner()).is_empty() }

    /// 按新的（或原来的）目录重新保存所有失败的截图。
    pub fn retry_save(&self, root: Option<PathBuf>) {
        let images: Vec<_> = std::mem::take(&mut *self.failed.lock().unwrap_or_else(|e| e.into_inner()));
        UNSAVED.store(0, Ordering::Release);
        for image in images {
            let image = Arc::try_unwrap(image).unwrap_or_else(|shared| SelectedImage { width: shared.width, height: shared.height, rgba: shared.rgba.clone(), confirmed_at: shared.confirmed_at, root: None });
            let image = SelectedImage { root: root.clone(), confirmed_at: Instant::now(), ..image };
            let _ = self.save_tx.send(Arc::new(image));
        }
    }

    /// 放弃所有未保存的截图。
    pub fn discard_failed(&self) {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).clear();
        UNSAVED.store(0, Ordering::Release);
    }

    /// 把最近一张未保存的截图复制到剪贴板，让用户在盘符丢失时也能拿走它。
    pub fn copy_failed(&self) {
        if let Some(image) = self.failed.lock().unwrap_or_else(|e| e.into_inner()).last().cloned() { let _ = self.copy_tx.send(image); }
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
    mut work: impl FnMut(&Arc<SelectedImage>) -> CaptureEvent + Send + 'static,
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

fn save_image(image: &Arc<SelectedImage>, failed: &Mutex<Vec<Arc<SelectedImage>>>) -> CaptureEvent {
    let fail = |error: String| {
        let mut list = failed.lock().unwrap_or_else(|e| e.into_inner());
        list.push(image.clone());
        UNSAVED.store(list.len(), Ordering::Release);
        timing(&format!("save_failed {error}"));
        CaptureEvent::SaveFailed { error, images: Arc::new(super::library::images(image.width, image.height, &image.rgba)), pending: list.len() }
    };
    let Some(root) = &image.root else {
        return fail("还没有设置截图保存位置：请选择一个文件夹".into());
    };
    match store::save_png(&image.rgba, image.width, image.height, root, chrono::Local::now().naive_local()) {
        Ok(path) => {
            let elapsed_ms = image.confirmed_at.elapsed().as_secs_f64() * 1000.0;
            timing(&format!("saved {}x{} confirm_to_png_ms={elapsed_ms:.3} path={}", image.width, image.height, path.display()));
            let images = Arc::new(super::library::images(image.width, image.height, &image.rgba));
            CaptureEvent::Saved {path, width:image.width, height:image.height, elapsed_ms, images}
        }
        Err(error) => fail(error),
    }
}

fn copy_image(image: &Arc<SelectedImage>) -> CaptureEvent {
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

/// 截图耗时记录：设置环境变量 `DAOZAI_SCREENSHOT_TIMING=1` 时打印到标准错误，平时不输出、不写文件。
pub(crate) fn timing(message: &str) {
    if std::env::var_os("DAOZAI_SCREENSHOT_TIMING").is_none() { return; }
    let mode = if cfg!(debug_assertions) { "debug" } else { "release" };
    eprintln!("screenshot: {} [{mode}] {message}", chrono::Local::now().to_rfc3339());
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
            CaptureEvent::Copied(Err("合成写盘失败".into()))
        }).unwrap();
        let copy = spawn_worker("test-screenshot-copy", ctx, events_tx, |image| {
            assert_eq!((image.width, image.height, image.rgba.len()), (2, 2, 16));
            CaptureEvent::Copied(if image.rgba[0] == 1 { Err("第一张合成复制失败".into()) } else { Ok(()) })
        }).unwrap();
        let image = |value| Arc::new(SelectedImage {width:2,height:2,rgba:vec![value;16],confirmed_at:Instant::now(),root:None});
        let first = image(1);
        save.send(first.clone()).unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        copy.send(first).unwrap();
        copy.send(image(2)).unwrap();
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::Copied(Err(error)) if error=="第一张合成复制失败"));
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::Copied(Ok(()))), "上一张失败不能阻止下一张复制，且不等待慢写盘");
        release.send(()).unwrap();
        assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), CaptureEvent::Copied(Err(error)) if error=="合成写盘失败"));
    }

    #[test]
    fn failed_saves_stay_in_memory_until_saved_elsewhere() {
        let failed: Mutex<Vec<Arc<SelectedImage>>> = Mutex::default();
        let image = Arc::new(SelectedImage {width:2,height:2,rgba:vec![7;16],confirmed_at:Instant::now(),root:None});
        let event = save_image(&image, &failed);
        assert!(matches!(event, CaptureEvent::SaveFailed {pending:1, ref images, ..} if (images.width, images.height)==(2,2)), "没有保存目录时保留图片");
        assert_eq!(failed.lock().unwrap().len(), 1);
        let root = std::env::temp_dir().join(format!("daozai-failed-save-{}", std::process::id()));
        let kept = failed.lock().unwrap().pop().unwrap();
        assert!(Arc::ptr_eq(&kept, &image), "保留的就是这张图");
        let retry = Arc::new(SelectedImage {width:2,height:2,rgba:kept.rgba.clone(),confirmed_at:Instant::now(),root:Some(root.clone())});
        assert!(matches!(save_image(&retry, &failed), CaptureEvent::Saved {..}), "另选目录后保存成功");
        assert!(failed.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
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
    #[ignore = "显式 release 基准，只在系统临时目录写合成 PNG；设置 DAOZAI_SCREENSHOT_TIMING=1 查看耗时"]
    fn stage_a_release_png_benchmark() {
        assert!(!cfg!(debug_assertions), "请使用 cargo test --release 运行性能测试");
        let directory = std::env::temp_dir().join(format!("daozai-screenshot-bench-{}", std::process::id()));
        for (width, height) in [(400u32,300u32),(3840,2160)] {
            let mut rgba = Vec::with_capacity((width*height*4) as usize);
            for y in 0..height {
                for x in 0..width { rgba.extend_from_slice(&[(x%256) as u8,(y%256) as u8,((x/8)^(y/8)) as u8,255]); }
            }
            let mut image = SelectedImage {width,height,rgba,confirmed_at:Instant::now(),root:None};
            for sample in 1..=5 {
                image.confirmed_at = Instant::now();
                let path = store::save_png(&image.rgba,image.width,image.height,&directory,chrono::Local::now().naive_local()).unwrap();
                let elapsed_ms = image.confirmed_at.elapsed().as_secs_f64()*1000.0;
                assert_eq!(image::image_dimensions(&path).unwrap(),(width,height));
                timing(&format!("bench_png synthetic sample={sample} size={width}x{height} encode_write_ms={elapsed_ms:.3} path={}",path.display()));
            }
        }
    }
}
