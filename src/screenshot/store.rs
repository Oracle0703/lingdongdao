//! 截图图库的存储：截图文件夹本身就是历史（设计文档第 5 节）。
//!
//! 目录布局：`<根目录>\YYYY-MM\截图_YYYYMMDD_HHMMSS_mmm[_n].png`。不另存记录文件；
//! 启动和打开截图页时扫描文件名得到历史，按文件名中的时间倒序排列。

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;

/// 未设置目录且 `E:\` 存在时使用的默认位置（用户确认的需求）。
pub const DEFAULT_DIR: &str = r"E:\截图";
const PREFIX: &str = "截图_";
const PART: &str = ".part";

/// 排序键：文件名中的本地时间（`YYYYMMDDHHMMSSmmm`）和重名序号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key { pub stamp: u64, pub sequence: u32 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry { pub path: PathBuf, pub key: Key }

/// 设置为空时：`E:\` 存在就用默认目录，否则返回 `None`，由界面提示选择目录。
pub fn resolve_root(setting: &str) -> Option<PathBuf> {
    let setting = setting.trim();
    if !setting.is_empty() { return Some(PathBuf::from(setting)); }
    Path::new(r"E:\").is_dir().then(|| PathBuf::from(DEFAULT_DIR))
}

pub fn month_dir(time: chrono::NaiveDateTime) -> String { time.format("%Y-%m").to_string() }

pub fn file_name(time: chrono::NaiveDateTime, sequence: u32) -> String {
    let stem = time.format("截图_%Y%m%d_%H%M%S_%3f");
    if sequence == 0 { format!("{stem}.png") } else { format!("{stem}_{sequence}.png") }
}

fn digits(text: &str, len: usize) -> Option<u64> {
    (text.len() == len && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// 只认本功能生成的文件名；用户自己放进来的图片不显示，也就不会被误删。
pub fn parse_name(name: &str) -> Option<Key> {
    let body = name.strip_prefix(PREFIX)?.strip_suffix(".png")?;
    let mut parts = body.split('_');
    let date = digits(parts.next()?, 8)?;
    let time = digits(parts.next()?, 6)?;
    let millis = digits(parts.next()?, 3)?;
    let sequence = match parts.next() {
        None => 0,
        Some(seq) if !seq.is_empty() && !seq.starts_with('0') && seq.len() <= 9 && seq.bytes().all(|b| b.is_ascii_digit()) => seq.parse().ok()?,
        Some(_) => return None,
    };
    if parts.next().is_some() { return None; }
    Some(Key { stamp: (date * 1_000_000 + time) * 1000 + millis, sequence })
}

fn is_month_dir(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 7 && b[4] == b'-' && b[..4].iter().chain(&b[5..]).all(u8::is_ascii_digit)
        && matches!(name[5..].parse::<u8>(), Ok(1..=12))
}

/// 列出根目录下各月份文件夹中的截图，最新的在前；顺带删除本功能中断写入留下的 `.part`。
/// 根目录不存在时返回空列表。不进入符号链接或目录联接。
pub fn scan(root: &Path) -> std::io::Result<Vec<Entry>> {
    let months = match fs::read_dir(root) {
        Ok(months) => months,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for month in months.flatten() {
        let Ok(kind) = month.file_type() else { continue };
        if !kind.is_dir() || kind.is_symlink() || !month.file_name().to_str().is_some_and(is_month_dir) { continue; }
        let Ok(files) = fs::read_dir(month.path()) else { continue };
        for file in files.flatten() {
            let Ok(kind) = file.file_type() else { continue };
            if !kind.is_file() { continue; }
            let name = file.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(key) = parse_name(name) {
                entries.push(Entry { path: file.path(), key });
            } else if name.strip_suffix(PART).and_then(parse_name).is_some() {
                let _ = fs::remove_file(file.path());
            }
        }
    }
    entries.sort_by(|a, b| b.key.cmp(&a.key));
    Ok(entries)
}

/// 保存一张截图：同目录写 `.part`，写完后不覆盖地改名为正式文件名。
/// 中途失败会删除 `.part`；进程中断留下的 `.part` 由下次 `scan` 清理。
pub fn save_png(rgba: &[u8], width: u32, height: u32, root: &Path, time: chrono::NaiveDateTime) -> Result<PathBuf, String> {
    // image 库遇到尺寸与像素数量不符会直接 panic，先检查。
    let expected = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(format!("截图像素数据与尺寸 {width} × {height} 不符"));
    }
    let directory = root.join(month_dir(time));
    fs::create_dir_all(&directory).map_err(|e| format!("无法创建截图目录 {}：{e}", directory.display()))?;
    for sequence in 0u32..1000 {
        let path = directory.join(file_name(time, sequence));
        if path.exists() { continue; }
        let mut part = path.clone().into_os_string();
        part.push(PART);
        let part = PathBuf::from(part);
        let file = match OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("无法写入截图 {}：{error}", part.display())),
        };
        let result = (|| -> Result<(), String> {
            let mut writer = BufWriter::new(file);
            PngEncoder::new_with_quality(&mut writer, CompressionType::Fast, FilterType::Adaptive)
                .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("PNG 编码失败：{e}"))?;
            writer.flush().map_err(|e| format!("PNG 写入失败：{e}"))?;
            writer.get_ref().sync_all().map_err(|e| format!("PNG 同步失败：{e}"))?;
            drop(writer);
            publish(&part, &path)
        })();
        if result.is_err() { let _ = fs::remove_file(&part); }
        result?;
        return Ok(path);
    }
    Err("同一毫秒内的截图重名序号已用完".into())
}

/// 改名但不覆盖已有文件。`std::fs::rename` 在 Windows 上会覆盖同名文件，不能用。
#[cfg(windows)]
fn publish(part: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let from: Vec<u16> = part.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { MoveFileExW(windows::core::PCWSTR(from.as_ptr()), windows::core::PCWSTR(to.as_ptr()), MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| format!("PNG 发布失败（未覆盖已有文件）：{e}"))
}

#[cfg(not(windows))]
fn publish(part: &Path, path: &Path) -> Result<(), String> {
    fs::hard_link(part, path).and_then(|_| fs::remove_file(part)).map_err(|e| format!("PNG 发布失败（未覆盖已有文件）：{e}"))
}

/// 删除前的归属检查：规范化后必须是 `<根目录>\YYYY-MM\` 下、符合本功能命名的普通文件。
/// 规范化会解析符号链接和目录联接，越界或经链接指向别处都会被拒绝。
pub fn owned_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let refuse = |reason: &str| Err(format!("拒绝删除 {}：{reason}", path.display()));
    let root = match fs::canonicalize(root) { Ok(root) => root, Err(_) => return refuse("截图目录不可用") };
    let file = match fs::canonicalize(path) { Ok(file) => file, Err(_) => return refuse("文件不存在") };
    let Some(name) = file.file_name().and_then(|n| n.to_str()) else { return refuse("文件名无效") };
    if parse_name(name).is_none() { return refuse("不是岛仔保存的截图"); }
    let Some(month) = file.parent() else { return refuse("位置无效") };
    if month.parent() != Some(root.as_path()) || !month.file_name().and_then(|n| n.to_str()).is_some_and(is_month_dir) {
        return refuse("不在截图目录的月份文件夹中");
    }
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.is_file() => Ok(file),
        _ => refuse("不是普通文件"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recycled { Done, Declined }

/// 移入回收站。`FOF_WANTNUKEWARNING`：盘上没有回收站或文件过大时由系统询问，
/// 用户拒绝就保留文件，绝不静默永久删除。
#[cfg(windows)]
pub fn recycle(root: &Path, path: &Path) -> Result<Recycled, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHFileOperationW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW};
    let file = owned_path(root, path)?;
    // SHFileOperation 不接受 `\\?\` 前缀，pFrom 以两个 0 结尾。
    let text = file.as_os_str().to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let from: Vec<u16> = std::ffi::OsStr::new(&text).encode_wide().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    if operation.fAnyOperationsAborted.as_bool() { return Ok(Recycled::Declined); }
    if code != 0 { return Err(format!("无法移入回收站（错误 {code:#x}）")); }
    if file.exists() { return Ok(Recycled::Declined); }
    Ok(Recycled::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn time(h: u32, m: u32, s: u32, ms: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 6).unwrap().and_hms_milli_opt(h, m, s, ms).unwrap()
    }

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("daozai-shots-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn names_round_trip_and_only_our_pattern_is_recognised() {
        let t = time(15, 30, 12, 328);
        assert_eq!(file_name(t, 0), "截图_20261106_153012_328.png");
        assert_eq!(file_name(t, 2), "截图_20261106_153012_328_2.png");
        assert_eq!(month_dir(t), "2026-11");
        assert_eq!(parse_name("截图_20261106_153012_328.png"), Some(Key { stamp: 20261106153012328, sequence: 0 }));
        assert_eq!(parse_name("截图_20261106_153012_328_2.png"), Some(Key { stamp: 20261106153012328, sequence: 2 }));
        for other in ["截图_20261106_153012_328.jpg", "截图_2026116_153012_328.png", "截图_20261106_153012_328_02.png",
            "截图_20261106_153012_328_.png", "截图_20261106_153012_328_1_2.png", "Screenshot 2026.png",
            "截图_20261106_153012_328.png.part", "截图_2026110x_153012_328.png"] {
            assert_eq!(parse_name(other), None, "{other}");
        }
        assert!(is_month_dir("2026-11") && !is_month_dir("2026-13") && !is_month_dir("2026-1") && !is_month_dir("202611-"));
    }

    #[test]
    fn scan_lists_newest_first_ignores_strangers_and_cleans_our_partial_files() {
        let root = temp_root("scan");
        for (month, name) in [("2026-10", "截图_20261031_235959_999.png"), ("2026-11", "截图_20261106_153012_328.png"),
            ("2026-11", "截图_20261106_153012_328_1.png"), ("2026-11", "截图_20261106_090000_000.png")] {
            fs::create_dir_all(root.join(month)).unwrap();
            fs::write(root.join(month).join(name), b"png").unwrap();
        }
        fs::write(root.join("2026-11").join("我的照片.png"), b"x").unwrap();
        fs::write(root.join("2026-11").join("截图_20261106_100000_000.png.part"), b"half").unwrap();
        fs::write(root.join("2026-11").join("别人的.part"), b"keep").unwrap();
        fs::create_dir_all(root.join("其他文件夹")).unwrap();
        fs::write(root.join("其他文件夹").join("截图_20261201_000000_000.png"), b"x").unwrap();
        fs::write(root.join("截图_20261202_000000_000.png"), b"x").unwrap();
        let names: Vec<_> = scan(&root).unwrap().iter().map(|e| e.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["截图_20261106_153012_328_1.png", "截图_20261106_153012_328.png", "截图_20261106_090000_000.png", "截图_20261031_235959_999.png"]);
        assert!(!root.join("2026-11").join("截图_20261106_100000_000.png.part").exists(), "本功能的 .part 被清理");
        assert!(root.join("2026-11").join("别人的.part").exists(), "不删除别人的文件");
        assert!(root.join("2026-11").join("我的照片.png").exists());
        assert!(scan(&root.join("不存在")).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_writes_month_folder_and_never_overwrites_same_millisecond() {
        let root = temp_root("save");
        let rgba = [10u8, 20, 30, 255].repeat(6);
        let t = time(15, 30, 12, 328);
        let first = save_png(&rgba, 3, 2, &root, t).unwrap();
        let second = save_png(&rgba, 3, 2, &root, t).unwrap();
        assert_eq!(first, root.join("2026-11").join("截图_20261106_153012_328.png"));
        assert_eq!(second, root.join("2026-11").join("截图_20261106_153012_328_1.png"));
        assert_eq!(image::image_dimensions(&first).unwrap(), (3, 2));
        assert_eq!(fs::read_dir(root.join("2026-11")).unwrap().count(), 2, "不留下 .part");
        let decoded = image::open(&second).unwrap().to_rgba8();
        assert_eq!(decoded.as_raw().as_slice(), rgba.as_slice());
        assert!(save_png(&rgba, 4, 2, &root, t).is_err(), "像素数量不符时报错");
        assert_eq!(fs::read_dir(root.join("2026-11")).unwrap().count(), 2, "失败时删除 .part");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_target_must_be_our_file_inside_a_month_folder() {
        let root = temp_root("owned");
        let month = root.join("2026-11");
        fs::create_dir_all(&month).unwrap();
        let ours = month.join("截图_20261106_153012_328.png");
        fs::write(&ours, b"png").unwrap();
        assert_eq!(owned_path(&root, &ours).unwrap(), fs::canonicalize(&ours).unwrap());
        let stranger = month.join("我的照片.png");
        fs::write(&stranger, b"x").unwrap();
        let loose = root.join("截图_20261106_153012_328.png");
        fs::write(&loose, b"x").unwrap();
        let outside = temp_root("outside").join("2026-11");
        fs::create_dir_all(&outside).unwrap();
        let elsewhere = outside.join("截图_20261106_153012_328.png");
        fs::write(&elsewhere, b"x").unwrap();
        for path in [&stranger, &loose, &elsewhere, &month.join("..").join("2026-11").join("不存在.png"), &month.join("截图_20261106_000000_000.png")] {
            assert!(owned_path(&root, path).is_err(), "{}", path.display());
        }
        let traversal = month.join("..").join("2026-11").join("截图_20261106_153012_328.png");
        assert!(owned_path(&root, &traversal).is_ok(), "带 .. 的写法规范化后仍在原位，可以接受");
        fs::remove_dir_all(outside.parent().unwrap()).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_root_follows_setting_then_e_drive() {
        assert_eq!(resolve_root(r"  D:\截图  "), Some(PathBuf::from(r"D:\截图")));
        let fallback = resolve_root("");
        if Path::new(r"E:\").is_dir() { assert_eq!(fallback, Some(PathBuf::from(DEFAULT_DIR))); } else { assert_eq!(fallback, None); }
    }
}
