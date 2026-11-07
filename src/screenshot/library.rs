//! 截图图库的后台服务：扫描目录、解码图片并缩成预览图和缩略图（跨平台，不接触窗口）。
//!
//! 两个工作线程共用一个后进先出的队列：用户快速切图时，最新的请求先处理；
//! 每个结果带上请求时的“代次”，界面据此丢弃过期结果（例如更换目录之后）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use eframe::egui::{self, ColorImage};

use super::store::{self, Entry};

/// 主预览图最大像素：岛上预览区约 387 × 130 点，按 200% 系统缩放再乘 150% 岛的大小留足。
pub const PREVIEW_MAX: (u32, u32) = (1280, 512);
/// 缩略图按“铺满裁切”生成的像素尺寸（岛上约 48 × 30 点）。
pub const THUMB_SIZE: (u32, u32) = (160, 100);

/// 一张截图解码后给界面用的两份小图，以及原图尺寸。
#[derive(Clone)]
pub struct Images { pub width: u32, pub height: u32, pub preview: ColorImage, pub thumb: ColorImage }

impl std::fmt::Debug for Images {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Images({} × {}, 预览 {:?})", self.width, self.height, self.preview.size)
    }
}

pub enum Request {
    Scan { root: PathBuf, generation: u64 },
    Decode { path: PathBuf, generation: u64 },
}

pub enum Response {
    Scanned { root: PathBuf, generation: u64, result: Result<Vec<Entry>, String> },
    Decoded { path: PathBuf, generation: u64, result: Result<Arc<Images>, String> },
}

struct Queue { items: Mutex<(VecDeque<Request>, bool)>, ready: Condvar }

pub struct Library {
    queue: Arc<Queue>,
    responses: mpsc::Receiver<Response>,
}

impl Library {
    /// `wake`：结果送达后调用（通常是请求重绘）。
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let queue = Arc::new(Queue { items: Mutex::new((VecDeque::new(), false)), ready: Condvar::new() });
        let (tx, responses) = mpsc::channel();
        let wake = Arc::new(wake);
        for index in 0..2 {
            let (queue, tx, wake) = (queue.clone(), tx.clone(), wake.clone());
            let _ = std::thread::Builder::new().name(format!("daozai-screenshot-library-{index}")).spawn(move || loop {
                let request = {
                    let mut items = queue.items.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if items.1 { return; }
                        if let Some(request) = items.0.pop_front() { break request; }
                        items = queue.ready.wait(items).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let response = match request {
                    Request::Scan { root, generation } => {
                        let result = store::scan(&root).map_err(|e| format!("无法读取截图目录 {}：{e}", root.display()));
                        Response::Scanned { root, generation, result }
                    }
                    Request::Decode { path, generation } => {
                        let result = decode(&path).map(Arc::new);
                        Response::Decoded { path, generation, result }
                    }
                };
                if tx.send(response).is_err() { return; }
                wake();
            });
        }
        Self { queue, responses }
    }

    /// 新请求排在最前，先于之前排队的请求处理。
    pub fn request(&self, request: Request) {
        let mut items = self.queue.items.lock().unwrap_or_else(|e| e.into_inner());
        items.0.push_front(request);
        self.queue.ready.notify_one();
    }

    /// 丢弃尚未开始的解码请求（例如更换目录时）。
    pub fn clear_decodes(&self) {
        let mut items = self.queue.items.lock().unwrap_or_else(|e| e.into_inner());
        items.0.retain(|request| matches!(request, Request::Scan { .. }));
    }

    pub fn poll(&self) -> Vec<Response> { self.responses.try_iter().collect() }
}

impl Drop for Library {
    fn drop(&mut self) {
        self.queue.items.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.queue.ready.notify_all();
    }
}

fn decode(path: &Path) -> Result<Images, String> {
    let image = image::open(path).map_err(|e| match e {
        image::ImageError::IoError(io) if io.kind() == std::io::ErrorKind::NotFound => "文件已移动或删除".to_string(),
        other => format!("无法读取这张截图：{other}"),
    })?.to_rgba8();
    Ok(images(image.width(), image.height(), image.as_raw()))
}

/// 由原图像素生成预览图（等比缩小到 `PREVIEW_MAX` 以内）和缩略图（铺满裁切到 `THUMB_SIZE`）。
/// 刚保存的截图直接用内存中的像素调用，不必再从磁盘解码。
pub fn images(width: u32, height: u32, rgba: &[u8]) -> Images {
    let source = image::RgbaImage::from_raw(width, height, rgba.to_vec()).unwrap_or_else(|| image::RgbaImage::new(1, 1));
    let (width, height) = source.dimensions();
    let scale = (PREVIEW_MAX.0 as f32 / width as f32).min(PREVIEW_MAX.1 as f32 / height as f32).min(1.0);
    let (pw, ph) = (((width as f32 * scale).round() as u32).max(1), ((height as f32 * scale).round() as u32).max(1));
    let preview = if (pw, ph) == (width, height) { source.clone() } else { image::imageops::thumbnail(&source, pw, ph) };
    let thumb = {
        let (tw, th) = THUMB_SIZE;
        let target = tw as f32 / th as f32;
        let (cw, ch) = if width as f32 / height as f32 > target {
            (((height as f32 * target).round() as u32).clamp(1, width), height)
        } else {
            (width, ((width as f32 / target).round() as u32).clamp(1, height))
        };
        let cropped = image::imageops::crop_imm(&source, (width - cw) / 2, (height - ch) / 2, cw, ch).to_image();
        if cw >= tw && ch >= th { image::imageops::thumbnail(&cropped, tw, th) }
        else { image::imageops::resize(&cropped, tw, th, image::imageops::FilterType::Triangle) }
    };
    let color = |image: &image::RgbaImage| ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw());
    Images { width, height, preview: color(&preview), thumb: color(&thumb) }
}

/// 界面里纹理的名称前缀，便于调试时辨认。
pub fn texture_name(path: &Path, kind: &str) -> String { format!("screenshot-{kind}-{}", path.display()) }

pub fn texture_options() -> egui::TextureOptions { egui::TextureOptions::LINEAR }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_keeps_aspect_and_thumbnail_fills_its_box() {
        for (w, h) in [(3840, 2160), (400, 300), (200, 2000), (3000, 40), (1, 1)] {
            let rgba = vec![200u8; (w * h * 4) as usize];
            let images = images(w, h, &rgba);
            let [pw, ph] = images.preview.size;
            assert!(pw as u32 <= PREVIEW_MAX.0 && ph as u32 <= PREVIEW_MAX.1, "{w}x{h} → {pw}x{ph}");
            let (src, dst) = (w as f32 / h as f32, pw as f32 / ph as f32);
            assert!((src - dst).abs() / src < 0.05 || pw == 1 || ph == 1, "保持比例：{w}x{h} → {pw}x{ph}");
            assert_eq!(images.thumb.size, [THUMB_SIZE.0 as usize, THUMB_SIZE.1 as usize]);
            assert_eq!((images.width, images.height), (w, h));
        }
        // 2 像素宽、2160 高的细条：不应生成上百 MB 的中间图（这里只验证结果尺寸）。
        let strip = images(2, 2160, &vec![3u8; 2 * 2160 * 4]);
        assert_eq!(strip.thumb.size, [THUMB_SIZE.0 as usize, THUMB_SIZE.1 as usize]);
        let small = images(400, 300, &vec![1u8; 400 * 300 * 4]);
        assert_eq!(small.preview.size, [400, 300], "小图不放大");
    }

    #[test]
    fn worker_scans_and_decodes_and_reports_missing_files() {
        let root = std::env::temp_dir().join(format!("daozai-library-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(9, 0, 0, 0).unwrap();
        let saved = store::save_png(&[9, 8, 7, 255].repeat(12), 4, 3, &root, time).unwrap();
        let library = Library::start(|| {});
        library.request(Request::Scan { root: root.clone(), generation: 3 });
        library.request(Request::Decode { path: saved.clone(), generation: 3 });
        library.request(Request::Decode { path: root.join("不存在.png"), generation: 3 });
        let mut responses = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while responses.len() < 3 && std::time::Instant::now() < deadline {
            responses.extend(library.poll());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(responses.len(), 3);
        for response in responses {
            match response {
                Response::Scanned { generation, result, .. } => { assert_eq!(generation, 3); assert_eq!(result.unwrap().len(), 1); }
                Response::Decoded { path, result, .. } if path == saved => { let images = result.unwrap(); assert_eq!((images.width, images.height), (4, 3)); }
                Response::Decoded { result, .. } => assert_eq!(result.err().unwrap(), "文件已移动或删除"),
            }
        }
        drop(library);
        std::fs::remove_dir_all(root).unwrap();
    }
}
