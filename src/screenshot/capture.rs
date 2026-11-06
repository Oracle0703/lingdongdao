//! Windows 原生截图：屏幕坐标和裁剪尺寸始终使用物理像素。
//!
//! 取帧时仅排除岛窗口；设置、邮件等其他窗口不改可见性。

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{timing, SelectedImage};
use super::selection::{self, Grip, Hit, PixelRect, SelectionMetrics, ToolbarLayout};

const CLASS: PCWSTR = w!("DaozaiForkScreenshotOverlay");
const TIMER: usize = 1;

impl PixelRect {
    fn win32(self) -> RECT { RECT {left:self.left, top:self.top, right:self.right, bottom:self.bottom} }
}

fn screen_to_local(point: (i32, i32), bounds: PixelRect) -> (i32, i32) {
    (point.0.saturating_sub(bounds.left), point.1.saturating_sub(bounds.top))
}

fn selection_rect(start: (i32, i32), end: (i32, i32), width: i32, height: i32) -> PixelRect {
    let (ax, ay) = (start.0.clamp(0, width), start.1.clamp(0, height));
    let (bx, by) = (end.0.clamp(0, width), end.1.clamp(0, height));
    PixelRect {left:ax.min(bx), top:ay.min(by), right:ax.max(bx), bottom:ay.max(by)}
}

fn byte_length(width: i32, height: i32) -> Result<usize, String> {
    if width <= 0 || height <= 0 { return Err("显示器尺寸无效".into()); }
    (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= isize::MAX as usize).ok_or_else(|| "截图尺寸溢出".into())
}

struct DpiGuard(DPI_AWARENESS_CONTEXT);
impl DpiGuard {
    fn new() -> Result<Self, String> {
        let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() { return Err("无法启用截图线程的物理像素 DPI 模式".into()); }
        Ok(Self(previous))
    }
}
impl Drop for DpiGuard { fn drop(&mut self) { unsafe { SetThreadDpiAwarenessContext(self.0); } } }

struct AffinityGuard { island: HWND, active: bool }
impl AffinityGuard {
    fn new(island: HWND) -> Result<Self, String> {
        unsafe { SetWindowDisplayAffinity(island, WDA_EXCLUDEFROMCAPTURE) }
            .map_err(|e| format!("无法从截图中排除岛窗口：{e}"))?;
        Ok(Self {island, active:true})
    }
    fn restore(&mut self) -> Result<(), String> {
        unsafe { SetWindowDisplayAffinity(self.island, WDA_NONE) }
            .map_err(|e| format!("无法还原岛窗口的取帧属性：{e}"))?;
        self.active = false;
        Ok(())
    }
}
impl Drop for AffinityGuard {
    fn drop(&mut self) { if self.active { unsafe { let _ = SetWindowDisplayAffinity(self.island, WDA_NONE); } } }
}

struct DesktopDc(HDC);
impl Drop for DesktopDc { fn drop(&mut self) { unsafe { ReleaseDC(None, self.0); } } }

/// 原始 DIB 只读取；暗帧在会话开始时生成一次，绘制缓冲仅覆盖 rcPaint。
struct Bitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}
impl Bitmap {
    unsafe fn new(reference: HDC, width: i32, height: i32) -> Result<Self, String> {
        byte_length(width, height)?;
        let dc = CreateCompatibleDC(reference);
        if dc.is_invalid() { return Err("创建截图内存 DC 失败".into()); }
        let info = BITMAPINFO {bmiHeader:BITMAPINFOHEADER {
            biSize:std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth:width, biHeight:-height,
            biPlanes:1, biBitCount:32, biCompression:BI_RGB.0, ..Default::default()
        }, ..Default::default()};
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(reference, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(error) => { let _ = DeleteDC(dc); return Err(format!("创建截图位图失败：{error}")); }
        };
        let previous = SelectObject(dc, bitmap);
        if previous.is_invalid() || previous.0 as isize == -1 || bits.is_null() {
            let _ = DeleteObject(bitmap); let _ = DeleteDC(dc);
            return Err("绑定截图位图失败".into());
        }
        Ok(Self {dc, bitmap, previous, bits:bits.cast(), width, height})
    }

    fn crop(&self, rect: PixelRect) -> Result<Vec<u8>, String> {
        let size = byte_length(rect.width(), rect.height())?;
        if rect.left < 0 || rect.top < 0 || rect.right > self.width || rect.bottom > self.height {
            return Err("裁剪区域超出显示器边界".into());
        }
        // 访问 DIB 内存前，等待 GDI 批处理写完。
        unsafe { let _ = GdiFlush(); }
        let source = unsafe { std::slice::from_raw_parts(self.bits, byte_length(self.width, self.height)?) };
        crop_bgra(source, self.width as usize, rect, size)
    }

    fn darkened(&self) -> Result<Self, String> {
        unsafe {
            if !GdiFlush().as_bool() { return Err("暗帧生成前 GDI 同步失败".into()); }
            let dark = Self::new(self.dc, self.width, self.height)?;
            let length = byte_length(self.width, self.height)?;
            let source = std::slice::from_raw_parts(self.bits, length);
            let target = std::slice::from_raw_parts_mut(dark.bits, length);
            // 与原先黑色遮罩 alpha=105 等价；保留原始帧用于选区和 PNG。
            for (from, to) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
                for channel in 0..3 { to[channel] = ((from[channel] as u16 * 150 + 127) / 255) as u8; }
                to[3] = 255;
            }
            Ok(dark)
        }
    }
}
impl Drop for Bitmap {
    fn drop(&mut self) { unsafe { SelectObject(self.dc, self.previous); let _ = DeleteObject(self.bitmap); let _ = DeleteDC(self.dc); } }
}

fn crop_bgra(source: &[u8], source_width: usize, rect: PixelRect, size: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| "截图裁剪内存不足".to_string())?;
    for y in rect.top as usize..rect.bottom as usize {
        let start = (y * source_width + rect.left as usize) * 4;
        let end = start + rect.width() as usize * 4;
        let row = source.get(start..end).ok_or_else(|| "截图像素数据不完整".to_string())?;
        for pixel in row.chunks_exact(4) { out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]); }
    }
    Ok(out)
}

struct Frame { bounds: PixelRect, image: Bitmap }

unsafe fn capture_frame(desktop: HDC, bounds: PixelRect) -> Result<(Frame, f64, f64), String> {
    let started = Instant::now();
    let image = Bitmap::new(desktop, bounds.width(), bounds.height())?;
    let blit_started = Instant::now();
    BitBlt(image.dc, 0, 0, image.width, image.height, desktop, bounds.left, bounds.top, SRCCOPY | CAPTUREBLT)
        .map_err(|e| format!("显示器 ({}, {}) 取帧失败：{e}", bounds.left, bounds.top))?;
    if !GdiFlush().as_bool() { return Err("截图 GDI 同步失败".into()); }
    let bitblt_ms = blit_started.elapsed().as_secs_f64() * 1000.0;
    Ok((Frame {bounds, image}, started.elapsed().as_secs_f64() * 1000.0, bitblt_ms))
}

unsafe fn monitors() -> Result<Vec<PixelRect>, String> {
    unsafe extern "system" fn collect(_: HMONITOR, _: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
        if !rect.is_null() {
            let rect = *rect;
            (*(data.0 as *mut Vec<PixelRect>)).push(PixelRect {left:rect.left, top:rect.top, right:rect.right, bottom:rect.bottom});
        }
        BOOL(1)
    }
    let mut list = Vec::new();
    if !EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut Vec<PixelRect> as isize)).as_bool() || list.is_empty() {
        return Err("无法枚举当前显示器".into());
    }
    Ok(list)
}

fn freeze(island: HWND, cancel: &AtomicBool) -> Result<Vec<Frame>, String> {
    unsafe {
        let bounds = monitors()?;
        let mut affinity = AffinityGuard::new(island)?;
        DwmFlush().map_err(|e| format!("等待桌面合成失败：{e}"))?;
        let desktop = DesktopDc(GetDC(None));
        if desktop.0.is_invalid() { return Err("无法获取桌面 DC".into()); }
        let mut frames = Vec::with_capacity(bounds.len());
        let mut timings = Vec::with_capacity(bounds.len());
        for (index, bounds) in bounds.into_iter().enumerate() {
            if cancel.load(Ordering::Acquire) { break; }
            let (frame, capture_ms, bitblt_ms) = capture_frame(desktop.0, bounds)?;
            timings.push(format!("frame monitor={} origin=({}, {}) size={}x{} capture_ms={capture_ms:.3} bitblt_ms={bitblt_ms:.3}", index + 1, bounds.left, bounds.top, frame.image.width, frame.image.height));
            frames.push(frame);
        }
        affinity.restore()?;
        // 还原取帧属性后再落日志，不让磁盘 I/O 拉长排除窗口的时间。
        for line in timings { timing(&line); }
        Ok(frames)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button { Cancel, Save }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection { monitor: usize, rect: PixelRect, adjustable: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisualSelection { selection: Selection, pressed: Option<Button> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    Draw { monitor: usize, start: (i32,i32) },
    Transform { monitor: usize, original: PixelRect, grip: Grip, start: (i32,i32), moved: bool },
    Button { monitor: usize, button: Button, inside: bool },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Interaction { selection: Option<Selection>, gesture: Option<Gesture>, last_click_inside: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action { Cancel, Save }

impl Interaction {
    fn visual(self) -> Option<VisualSelection> {
        self.selection.map(|selection| VisualSelection {selection, pressed:match self.gesture {
            Some(Gesture::Button {button,inside:true,..}) => Some(button), _ => None,
        }})
    }
    fn hit(self, monitor: usize, point: (i32,i32), bounds: PixelRect, metrics: SelectionMetrics) -> Hit {
        if !bounds.contains(point) { return Hit::Outside; }
        self.selection.filter(|selected|selected.monitor == monitor && selected.adjustable)
            .map(|selected|selection::hit_test(selected.rect,point,bounds,metrics)).unwrap_or(Hit::Outside)
    }
    fn pointer_down(&mut self, monitor: usize, point: (i32,i32), bounds: PixelRect, metrics: SelectionMetrics) {
        if self.gesture.is_some() { return; }
        self.last_click_inside = false;
        let hit = self.hit(monitor,point,bounds,metrics);
        match hit {
            Hit::Cancel | Hit::Save => self.gesture = Some(Gesture::Button {monitor,button:if hit == Hit::Cancel {Button::Cancel}else{Button::Save},inside:true}),
            Hit::Grip(grip) => self.gesture = self.selection.map(|selected|Gesture::Transform {monitor,original:selected.rect,grip,start:point,moved:false}),
            Hit::Inside => self.gesture = self.selection.map(|selected|Gesture::Transform {monitor,original:selected.rect,grip:Grip::Move,start:point,moved:false}),
            Hit::Toolbar => {},
            Hit::Outside => {
                self.selection = Some(Selection {monitor,rect:selection_rect(point,point,bounds.width(),bounds.height()),adjustable:false});
                self.gesture = Some(Gesture::Draw {monitor,start:point});
            }
        }
    }
    fn pointer_move(&mut self, monitor: usize, point: (i32,i32), bounds: PixelRect, metrics: SelectionMetrics) {
        match self.gesture {
            Some(Gesture::Draw {monitor:locked,start}) if locked == monitor => {
                self.selection = Some(Selection {monitor,rect:selection_rect(start,point,bounds.width(),bounds.height()),adjustable:false});
            }
            Some(Gesture::Transform {monitor:locked,original,grip,start,moved}) if locked == monitor => {
                let delta = (point.0.saturating_sub(start.0),point.1.saturating_sub(start.1));
                // 整体移动保留少量点击抖动；一旦越过阈值便不可逆标记为拖动，防止拖回原位被当成双击。
                let moved=moved || grip != Grip::Move || delta.0.unsigned_abs().max(delta.1.unsigned_abs()) >= (metrics.hit_padding/2).max(1) as u32;
                self.gesture=Some(Gesture::Transform {monitor:locked,original,grip,start,moved});
                self.selection = Some(Selection {monitor,rect:if moved {selection::apply_drag(original,grip,delta,bounds)}else{original},adjustable:true});
            }
            Some(Gesture::Button {monitor:locked,button,..}) => {
                let hit = self.hit(monitor,point,bounds,metrics);
                let inside = locked == monitor && matches!((button,hit),(Button::Cancel,Hit::Cancel)|(Button::Save,Hit::Save));
                self.gesture = Some(Gesture::Button {monitor:locked,button,inside});
            }
            _ => {},
        }
    }
    fn pointer_up(&mut self, monitor: usize, point: (i32,i32), bounds: PixelRect, metrics: SelectionMetrics) -> Option<Action> {
        self.pointer_move(monitor,point,bounds,metrics);
        // 必须在调用原生 ReleaseCapture 前清空 gesture，正常释放不触发取消。
        let gesture = self.gesture.take();
        self.last_click_inside = false;
        match gesture {
            Some(Gesture::Draw {monitor:locked,..}) if locked == monitor => {
                if let Some(selected) = self.selection.as_mut() { selected.adjustable = true; }
            }
            Some(Gesture::Transform {monitor:locked,grip:Grip::Move,moved,..}) if locked == monitor => {
                self.last_click_inside = !moved && self.hit(monitor,point,bounds,metrics)==Hit::Inside;
            }
            Some(Gesture::Button {monitor:locked,button,inside:true}) if locked == monitor => {
                return match button { Button::Cancel => Some(Action::Cancel), Button::Save => self.confirmable().map(|_|Action::Save) };
            }
            _ => {},
        }
        None
    }
    fn confirmable(self) -> Option<Selection> {
        self.selection.filter(|selected|selected.adjustable && selected.rect.valid() && self.gesture.is_none())
    }
    fn double_click(self, monitor: usize, point: (i32,i32), bounds: PixelRect, metrics: SelectionMetrics) -> bool {
        self.gesture.is_none() && self.last_click_inside && self.confirmable().is_some()
            && self.hit(monitor,point,bounds,metrics) == Hit::Inside
    }
}

#[derive(Clone, Copy, Debug)]
enum Outcome { Cancelled, Failed(&'static str), Selected { monitor: usize, rect: PixelRect, confirmed_at: Instant } }

#[derive(Clone, Copy, Debug)]
struct OverlayMetrics { font_height: i32, margin: i32, gap: i32, padding_x: i32, padding_y: i32, border: i32, selection: SelectionMetrics }
impl OverlayMetrics {
    fn for_dpi(dpi: u32) -> Self {
        let scale = |logical: i32| ((logical as i64 * dpi.max(96) as i64 + 48) / 96).clamp(1, i32::MAX as i64) as i32;
        Self {font_height:scale(16), margin:scale(24), gap:scale(6), padding_x:scale(8), padding_y:scale(4), border:scale(2),
            selection:SelectionMetrics {handle_size:scale(8),hit_padding:scale(6),toolbar_gap:scale(8),button_width:scale(88),button_height:scale(34),button_gap:scale(4),padding:scale(6)}}
    }
}

struct OverlayFont(HFONT);
impl Drop for OverlayFont { fn drop(&mut self) { unsafe { let _ = DeleteObject(self.0); } } }

struct OverlayStyle { metrics: OverlayMetrics, font: OverlayFont }
impl OverlayStyle {
    fn new(dpi: u32) -> Result<Self, String> {
        let metrics = OverlayMetrics::for_dpi(dpi);
        let font = unsafe { CreateFontW(-metrics.font_height, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32, DEFAULT_PITCH.0 as u32, w!("Microsoft YaHei")) };
        if font.is_invalid() { return Err("创建截图提示字体失败".into()); }
        Ok(Self {metrics, font:OverlayFont(font)})
    }

    fn measure(&self, dc: HDC, text: &[u16]) -> Result<SIZE, String> {
        unsafe {
            let previous = SelectObject(dc, self.font.0);
            let mut size = SIZE::default();
            let success = GetTextExtentPoint32W(dc, text, &mut size).as_bool();
            SelectObject(dc, previous);
            if !success { return Err("测量截图提示文字失败".into()); }
            Ok(size)
        }
    }
}

struct OverlayLayout {
    selection: Option<PixelRect>, handles: Vec<PixelRect>, toolbar: Option<ToolbarLayout>,
    save_enabled: bool, pressed: Option<Button>, label: PixelRect, text: Vec<u16>,
}

fn overlay_layout(frame: &Frame, style: &OverlayStyle, visual: Option<VisualSelection>) -> Result<OverlayLayout, String> {
    let (width, height) = (frame.image.width, frame.image.height);
    let bounds = PixelRect {left:0,top:0,right:width,bottom:height};
    let selection = visual.map(|visual|visual.selection.rect);
    let text: Vec<u16> = selection.map(|rect| format!("{} × {} px", rect.width(), rect.height()))
        .unwrap_or_else(|| "拖动框选 · Esc / 右键取消".into()).encode_utf16().collect();
    let metrics = style.metrics;
    let adjusting = visual.filter(|visual|visual.selection.adjustable);
    let handles:Vec<PixelRect> = adjusting.map(|visual|selection::handles(visual.selection.rect,metrics.selection).into_iter().map(|(_,rect)|rect).collect()).unwrap_or_default();
    let toolbar = adjusting.map(|visual|selection::toolbar_layout(visual.selection.rect,bounds,metrics.selection));
    let text_size = style.measure(frame.image.dc, &text)?;
    let label_width = (text_size.cx + metrics.padding_x * 2).min(width);
    let label_height = (text_size.cy + metrics.padding_y * 2).min(height);
    let (x, y) = match selection {
        Some(rect) => (rect.left, if rect.top >= label_height + metrics.gap {rect.top-label_height-metrics.gap} else {rect.bottom+metrics.gap}),
        None => (metrics.margin, metrics.margin),
    };
    let place = |x:i32,y:i32| {
        let left=x.clamp(0,width-label_width); let top=y.clamp(0,height-label_height);
        PixelRect {left,top,right:left+label_width,bottom:top+label_height}
    };
    let mut label = place(x,y);
    if let (Some(rect),Some(toolbar)) = (selection,toolbar) {
        // 工具栏位置遵循固定规则；只挪标签，同时留开控制点和按钮周围的间距。
        let clearance=metrics.selection.handle_size/2+metrics.gap;
        let obstacles:Vec<PixelRect>=handles.iter().map(|handle|handle.expand(metrics.gap))
            .chain(std::iter::once(toolbar.bounds.expand(metrics.gap))).collect();
        let free=|candidate:PixelRect|obstacles.iter().all(|obstacle|candidate.intersect(*obstacle).is_none());
        if !free(label) {
            let candidates=[
                place(rect.left,rect.top-label_height-clearance),
                place(rect.left,rect.bottom+clearance),
                place(rect.right+clearance,rect.top),
                place(rect.left-label_width-clearance,rect.top),
                place(toolbar.bounds.left,toolbar.bounds.top-label_height-metrics.gap),
                place(toolbar.bounds.left,toolbar.bounds.bottom+metrics.gap),
                place(rect.left+clearance,rect.top+clearance),
                place(rect.left+clearance,rect.bottom-label_height-clearance),
                place(rect.right-label_width-clearance,rect.top+clearance),
                place(rect.right-label_width-clearance,rect.bottom-label_height-clearance),
            ];
            if let Some(candidate)=candidates.into_iter().find(|candidate|free(*candidate)) {
                label=candidate;
            } else {
                // 贴边的窄长选区可能使常规候选都被 clamp；沿障碍边缘寻找仍在屏内的空位。
                let xs:Vec<i32>=std::iter::once(0).chain(std::iter::once(width-label_width))
                    .chain(obstacles.iter().flat_map(|obstacle|[obstacle.left-label_width,obstacle.right])).collect();
                let ys:Vec<i32>=std::iter::once(0).chain(std::iter::once(height-label_height))
                    .chain(obstacles.iter().flat_map(|obstacle|[obstacle.top-label_height,obstacle.bottom])).collect();
                if let Some(candidate)=ys.iter().flat_map(|&y|xs.iter().map(move |&x|place(x,y))).find(|candidate|free(*candidate)) {label=candidate;}
            }
        }
    }
    Ok(OverlayLayout {selection,handles,toolbar,save_enabled:selection.is_some_and(PixelRect::valid),pressed:visual.and_then(|v|v.pressed),label,text})
}

fn changed_monitors(before: Option<VisualSelection>, after: Option<VisualSelection>) -> Vec<usize> {
    if before == after { return Vec::new(); }
    let mut indices = Vec::with_capacity(2);
    if let Some(visual) = before { indices.push(visual.selection.monitor); }
    if let Some(visual) = after { if !indices.contains(&visual.selection.monitor) { indices.push(visual.selection.monitor); } }
    indices
}

fn dirty_region(before: &OverlayLayout, after: &OverlayLayout, metrics: OverlayMetrics, bounds: PixelRect) -> Option<PixelRect> {
    let mut dirty = before.label.union(after.label);
    for rect in [before.selection, after.selection].into_iter().flatten() {
        dirty = dirty.union(rect.expand(metrics.selection.handle_size/2+metrics.border));
    }
    for rect in before.handles.iter().chain(&after.handles) { dirty=dirty.union(*rect); }
    for toolbar in [before.toolbar,after.toolbar].into_iter().flatten() { dirty=dirty.union(toolbar.bounds); }
    dirty.intersect(bounds)
}

fn message_point(lp: LPARAM) -> (i32,i32) {
    // 鼠标消息中的有符号客户区坐标是该次事件的位置，不能用稍后的 GetCursorPos 代替。
    ((lp.0 as u16) as i16 as i32, ((lp.0 >> 16) as u16) as i16 as i32)
}

fn cursor_for_hit(hit: Hit) -> PCWSTR {
    match hit {
        Hit::Grip(Grip::NorthWest|Grip::SouthEast) => IDC_SIZENWSE,
        Hit::Grip(Grip::NorthEast|Grip::SouthWest) => IDC_SIZENESW,
        Hit::Grip(Grip::East|Grip::West) => IDC_SIZEWE,
        Hit::Grip(Grip::North|Grip::South) => IDC_SIZENS,
        Hit::Grip(Grip::Move)|Hit::Inside => IDC_SIZEALL,
        Hit::Cancel|Hit::Save => IDC_HAND,
        Hit::Outside|Hit::Toolbar => IDC_CROSS,
    }
}

fn transformed_grip(grip: Grip, original: PixelRect, start: (i32,i32), point: (i32,i32)) -> Grip {
    let (dx,dy)=(point.0 as i64-start.0 as i64,point.1 as i64-start.1 as i64);
    let horizontal=match grip {
        Grip::NorthWest|Grip::West|Grip::SouthWest => if original.left as i64+dx > original.right as i64 {1}else{-1},
        Grip::NorthEast|Grip::East|Grip::SouthEast => if original.right as i64+dx < original.left as i64 {-1}else{1},
        _=>0,
    };
    let vertical=match grip {
        Grip::NorthWest|Grip::North|Grip::NorthEast => if original.top as i64+dy > original.bottom as i64 {1}else{-1},
        Grip::SouthWest|Grip::South|Grip::SouthEast => if original.bottom as i64+dy < original.top as i64 {-1}else{1},
        _=>0,
    };
    match (horizontal,vertical) {(-1,-1)=>Grip::NorthWest,(0,-1)=>Grip::North,(1,-1)=>Grip::NorthEast,
        (1,0)=>Grip::East,(1,1)=>Grip::SouthEast,(0,1)=>Grip::South,(-1,1)=>Grip::SouthWest,(-1,0)=>Grip::West,_=>Grip::Move}
}

unsafe fn fill(dc: HDC, rect: PixelRect, color: COLORREF) -> Result<(),String> {
    SetDCBrushColor(dc,color);
    if FillRect(dc,&rect.win32(),HBRUSH(GetStockObject(DC_BRUSH).0)) == 0 { return Err("绘制截图控件失败".into()); }
    Ok(())
}

unsafe fn draw_text(dc: HDC, style: &OverlayStyle, text: &[u16], rect: PixelRect, centered: bool, color: COLORREF) -> Result<(),String> {
    let size=style.measure(dc,text)?;
    let (x,y)=if centered {(rect.left+(rect.width()-size.cx)/2,rect.top+(rect.height()-size.cy)/2)}
        else {(rect.left+style.metrics.padding_x,rect.top+style.metrics.padding_y)};
    SetBkMode(dc,TRANSPARENT); SetTextColor(dc,color);
    let previous=SelectObject(dc,style.font.0);
    let success=ExtTextOutW(dc,x,y,ETO_CLIPPED,Some(&rect.win32()),PCWSTR(text.as_ptr()),text.len() as u32,None).as_bool();
    SelectObject(dc,previous);
    if !success {return Err("绘制截图提示文字失败".into());}
    Ok(())
}

/// 所有合成均在小缓冲里完成，返回后只需一次 BitBlt 呈现；原帧不会被修改。
unsafe fn render_dirty(frame: &Frame, dark: &Bitmap, style: &OverlayStyle, layout: &OverlayLayout, dirty: PixelRect) -> Result<Bitmap, String> {
    let buffer = Bitmap::new(frame.image.dc, dirty.width(), dirty.height())?;
    BitBlt(buffer.dc, 0, 0, dirty.width(), dirty.height(), dark.dc, dirty.left, dirty.top, SRCCOPY)
        .map_err(|e| format!("绘制截图暗帧失败：{e}"))?;
    if let Some(rect) = layout.selection.filter(|rect| rect.width() > 0 && rect.height() > 0 && rect.expand(style.metrics.border).intersect(dirty).is_some()) {
        if let Some(visible) = rect.intersect(dirty) {
            BitBlt(buffer.dc, visible.left-dirty.left, visible.top-dirty.top, visible.width(), visible.height(), frame.image.dc, visible.left, visible.top, SRCCOPY)
                .map_err(|e| format!("绘制截图选区失败：{e}"))?;
        }
        let pen = CreatePen(PS_SOLID, style.metrics.border, COLORREF(0x00ffb34d));
        if pen.is_invalid() { return Err("创建截图选区边框失败".into()); }
        let old_pen = SelectObject(buffer.dc, pen);
        let old_brush = SelectObject(buffer.dc, GetStockObject(NULL_BRUSH));
        let rect = rect.offset(-dirty.left, -dirty.top);
        let success = Rectangle(buffer.dc, rect.left, rect.top, rect.right, rect.bottom).as_bool();
        SelectObject(buffer.dc, old_pen); SelectObject(buffer.dc, old_brush); let _ = DeleteObject(pen);
        if !success { return Err("绘制截图选区边框失败".into()); }
    }
    if layout.label.intersect(dirty).is_some() {
        let label=layout.label.offset(-dirty.left,-dirty.top);
        fill(buffer.dc,label,COLORREF(0x00202020))?;
        draw_text(buffer.dc,style,&layout.text,label,false,COLORREF(0x00ffffff))?;
    }
    for handle in &layout.handles {
        if handle.intersect(dirty).is_some() {
            let handle=handle.offset(-dirty.left,-dirty.top);
            fill(buffer.dc,handle,COLORREF(0x00ffb34d))?;
            if let Some(inner)=handle.expand(-style.metrics.border).intersect(handle) {fill(buffer.dc,inner,COLORREF(0x00ffffff))?;}
        }
    }
    if let Some(toolbar)=layout.toolbar.filter(|toolbar|toolbar.bounds.intersect(dirty).is_some()) {
        fill(buffer.dc,toolbar.bounds.offset(-dirty.left,-dirty.top),COLORREF(0x00202020))?;
        for (button,rect,label) in [(Button::Cancel,toolbar.cancel,"✕ 取消"),(Button::Save,toolbar.save,"✓ 保存")] {
            if rect.intersect(dirty).is_none() {continue;}
            let enabled=button==Button::Cancel || layout.save_enabled;
            let background=if enabled && layout.pressed==Some(button) {0x00505050}else{0x00343434};
            let rect=rect.offset(-dirty.left,-dirty.top);
            fill(buffer.dc,rect,COLORREF(background))?;
            let text:Vec<u16>=label.encode_utf16().collect();
            draw_text(buffer.dc,style,&text,rect,true,COLORREF(if enabled {0x00ffffff}else{0x007b7b7b}))?;
        }
    }
    Ok(buffer)
}

struct Session<'a> {
    frames: Vec<Frame>,
    dark_frames: Vec<Bitmap>,
    styles: RefCell<Vec<OverlayStyle>>,
    windows: RefCell<Vec<HWND>>,
    interaction: Cell<Interaction>,
    outcome: Cell<Option<Outcome>>,
    armed: Cell<bool>,
    cancel: &'a AtomicBool,
    previous_foreground: HWND,
}

impl Session<'_> {
    fn finish(&self, result: Outcome) {
        if self.outcome.get().is_none() { self.outcome.set(Some(result)); }
    }
    fn set_interaction(&self, after: Interaction) {
        let before = self.interaction.replace(after).visual();
        let after = after.visual();
        for index in changed_monitors(before, after) {
            let frame = &self.frames[index];
            let styles = self.styles.borrow();
            let style = &styles[index];
            let layouts = overlay_layout(frame, style, before.filter(|visual| visual.selection.monitor == index))
                .and_then(|old| overlay_layout(frame, style, after.filter(|visual| visual.selection.monitor == index)).map(|new| (old,new)));
            let Ok((old, new)) = layouts else { self.finish(Outcome::Failed("计算截图绘制区域失败")); return; };
            let bounds = PixelRect {left:0,top:0,right:frame.image.width,bottom:frame.image.height};
            if let Some(dirty) = dirty_region(&old, &new, style.metrics, bounds) {
                let hwnd = self.windows.borrow()[index];
                unsafe { let _ = InvalidateRect(hwnd, Some(&dirty.win32()), false); }
            }
        }
    }
    fn bounds(&self, index: usize) -> PixelRect {
        PixelRect {left:0,top:0,right:self.frames[index].image.width,bottom:self.frames[index].image.height}
    }
    fn pointer_down(&self, index: usize, point: (i32,i32)) -> bool {
        if self.outcome.get().is_some() {return false;}
        let mut interaction=self.interaction.get();
        interaction.pointer_down(index,point,self.bounds(index),self.styles.borrow()[index].metrics.selection);
        let capture=interaction.gesture.is_some();
        self.set_interaction(interaction);
        capture
    }
    fn pointer_move(&self, index: usize, point: (i32,i32)) {
        if self.outcome.get().is_some() {return;}
        let mut interaction=self.interaction.get();
        interaction.pointer_move(index,point,self.bounds(index),self.styles.borrow()[index].metrics.selection);
        self.set_interaction(interaction);
    }
    fn pointer_up(&self, index: usize, point: (i32,i32)) {
        if self.outcome.get().is_some() {return;}
        let confirmed_at=Instant::now();
        let mut interaction=self.interaction.get();
        let action=interaction.pointer_up(index,point,self.bounds(index),self.styles.borrow()[index].metrics.selection);
        self.set_interaction(interaction);
        match action {Some(Action::Cancel)=>self.finish(Outcome::Cancelled),Some(Action::Save)=>self.confirm_at(confirmed_at),None=>{}}
    }
    fn confirm(&self) {
        self.confirm_at(Instant::now());
    }
    fn confirm_at(&self, confirmed_at: Instant) {
        if let Some(selected)=self.interaction.get().confirmable() {
            self.finish(Outcome::Selected {monitor:selected.monitor,rect:selected.rect,confirmed_at});
        }
    }
    fn double_click(&self, index: usize, point: (i32,i32)) -> bool {
        let confirmed_at=Instant::now();
        let accepted=self.interaction.get().double_click(index,point,self.bounds(index),self.styles.borrow()[index].metrics.selection);
        if accepted {self.confirm_at(confirmed_at);}
        accepted
    }
    unsafe fn set_cursor(&self, index: usize, point: (i32,i32)) {
        let interaction=self.interaction.get();
        let hit=match interaction.gesture {
            Some(Gesture::Transform {original,grip,start,..}) => Hit::Grip(transformed_grip(grip,original,start,point)),
            Some(Gesture::Draw {..}) => Hit::Outside,
            _ => interaction.hit(index,point,self.bounds(index),self.styles.borrow()[index].metrics.selection),
        };
        if let Ok(cursor)=LoadCursorW(None,cursor_for_hit(hit)) {SetCursor(cursor);}
    }
    fn own_window(&self, hwnd: HWND) -> bool { self.windows.borrow().contains(&hwnd) }
    fn cursor(&self, index: usize) -> Option<(i32, i32)> {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() { return None; }
        let frame = &self.frames[index];
        Some(screen_to_local((point.x,point.y),frame.bounds))
    }
    unsafe fn paint(&self, index: usize, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let frame = &self.frames[index];
        let bounds = PixelRect {left:0,top:0,right:frame.image.width,bottom:frame.image.height};
        let dirty = PixelRect {left:ps.rcPaint.left,top:ps.rcPaint.top,right:ps.rcPaint.right,bottom:ps.rcPaint.bottom}.intersect(bounds);
        let styles = self.styles.borrow();
        if let (Some(dirty), Some(style)) = (dirty, styles.get(index)) {
            let result = overlay_layout(frame, style, self.interaction.get().visual().filter(|visual| visual.selection.monitor == index))
                .and_then(|layout| render_dirty(frame, &self.dark_frames[index], style, &layout, dirty))
                .and_then(|buffer| BitBlt(dc, dirty.left, dirty.top, dirty.width(), dirty.height(), buffer.dc, 0, 0, SRCCOPY).map_err(|e|e.to_string()));
            if result.is_err() { self.finish(Outcome::Failed("绘制截图遮罩失败")); }
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.armed.set(false);
        unsafe {
            let restore_focus = self.own_window(GetForegroundWindow());
            let release_capture = self.own_window(GetCapture());
            // 先解除所有窗口的回调指针，再触发 ReleaseCapture/DestroyWindow 的同步消息。
            for &hwnd in self.windows.borrow().iter() {
                let _ = KillTimer(hwnd, TIMER);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            if release_capture { let _ = ReleaseCapture(); }
            for &hwnd in self.windows.borrow().iter() {
                let _ = DestroyWindow(hwnd);
            }
            if restore_focus && IsWindow(self.previous_foreground).as_bool() { let _ = SetForegroundWindow(self.previous_foreground); }
        }
    }
}

struct WindowData<'a> { session: *const Session<'a>, index: usize }

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowData<'_>;
    if data.is_null() { return DefWindowProcW(hwnd, msg, wp, lp); }
    let data = &*data;
    let session = &*data.session;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => { session.paint(data.index, hwnd); LRESULT(0) }
        WM_LBUTTONDOWN if session.outcome.get().is_none() => {
            if session.pointer_down(data.index,message_point(lp)) {SetCapture(hwnd);}
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let point=message_point(lp);
            session.pointer_move(data.index,point);
            session.set_cursor(data.index,point);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            session.pointer_up(data.index,message_point(lp));
            if GetCapture()==hwnd {let _=ReleaseCapture();}
            session.set_cursor(data.index,message_point(lp));
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let point=message_point(lp);
            if !session.double_click(data.index,point) && session.pointer_down(data.index,point) {SetCapture(hwnd);}
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Some(point)=session.cursor(data.index) {session.set_cursor(data.index,point);return LRESULT(1);}
            DefWindowProcW(hwnd,msg,wp,lp)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if wp.0 == 0x0d => {session.confirm();LRESULT(0)}
        WM_KEYDOWN | WM_SYSKEYDOWN if wp.0 == 0x1b => { session.finish(Outcome::Cancelled); LRESULT(0) }
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_CANCELMODE | WM_CLOSE | WM_DISPLAYCHANGE | WM_DPICHANGED | WM_ENDSESSION => {
            session.finish(Outcome::Cancelled); LRESULT(0)
        }
        WM_CAPTURECHANGED if session.interaction.get().gesture.is_some() => { session.finish(Outcome::Cancelled); LRESULT(0) }
        WM_TIMER if wp.0 == TIMER => {
            // 切到其他窗口（含锁屏安全桌面）即取消，避免留下覆盖层。
            if session.cancel.load(Ordering::Acquire) || (session.armed.get() && !session.own_window(GetForegroundWindow())) { session.finish(Outcome::Cancelled); }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub(crate) fn select(island: HWND, cancel: &AtomicBool) -> Result<Option<SelectedImage>, String> {
    let _dpi = DpiGuard::new()?;
    let previous_foreground = unsafe { GetForegroundWindow() };
    let frames = freeze(island, cancel)?;
    if cancel.load(Ordering::Acquire) { return Ok(None); }
    unsafe {
        let module = GetModuleHandleW(None).map_err(|e| format!("截图模块句柄读取失败：{e}"))?;
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {style:CS_DBLCLKS,lpfnWndProc:Some(wndproc), hInstance:instance, hCursor:LoadCursorW(None, IDC_CROSS).map_err(|e| e.to_string())?, lpszClassName:CLASS, ..Default::default()};
        // 类在进程生命周期内复用；不注销其他截图会话共享的类。
        let _ = RegisterClassW(&class);
        // 排除岛的短窗口已结束；此后只处理内存快照，每屏暗帧仅生成一次。
        let mut dark_frames = Vec::with_capacity(frames.len());
        for frame in &frames {
            if cancel.load(Ordering::Acquire) { return Ok(None); }
            dark_frames.push(frame.image.darkened()?);
        }
        if cancel.load(Ordering::Acquire) { return Ok(None); }
        // Box 保证每个窗口 USERDATA 在 Vec 扩容时也保持稳定。
        // 声明顺序保证任意 ? 提前返回也先销毁 session 的窗口，再释放 USERDATA。
        let mut window_data: Vec<Box<WindowData<'_>>> = Vec::new();
        let session = Box::new(Session {frames, dark_frames, styles:RefCell::new(Vec::new()), windows:RefCell::new(Vec::new()), interaction:Cell::new(Interaction::default()), outcome:Cell::new(None), armed:Cell::new(false), cancel, previous_foreground});
        for (index, frame) in session.frames.iter().enumerate() {
            let data = Box::new(WindowData {session:&*session, index});
            let hwnd = match CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, CLASS, w!("岛仔区域截图"), WS_POPUP,
                frame.bounds.left, frame.bounds.top, frame.image.width, frame.image.height,
                None, None, instance, Some((&*data as *const WindowData<'_>).cast())) {
                Ok(hwnd) => hwnd,
                Err(error) => { drop(session); return Err(format!("创建截图遮罩失败：{error}")); }
            };
            session.windows.borrow_mut().push(hwnd);
            window_data.push(data);
            session.styles.borrow_mut().push(OverlayStyle::new(GetDpiForWindow(hwnd))?);
            if SetTimer(hwnd, TIMER, 30, None) == 0 { drop(session); return Err("创建截图取消定时器失败".into()); }
        }
        if cancel.load(Ordering::Acquire) { drop(session); return Ok(None); }
        for &hwnd in session.windows.borrow().iter() { let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE); }
        let mut cursor = POINT::default(); let _ = GetCursorPos(&mut cursor);
        let active_index = session.frames.iter().position(|f| cursor.x >= f.bounds.left && cursor.x < f.bounds.right && cursor.y >= f.bounds.top && cursor.y < f.bounds.bottom).unwrap_or(0);
        let active = session.windows.borrow()[active_index];
        let _ = SetForegroundWindow(active);
        if !session.own_window(GetForegroundWindow()) { drop(session); return Err("无法激活截图遮罩；请通过托盘菜单重试".into()); }
        session.armed.set(true);
        let mut message = MSG::default();
        while session.outcome.get().is_none() {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == -1 { drop(session); return Err("截图窗口消息循环失败".into()); }
            if result == 0 { session.finish(Outcome::Cancelled); break; }
            let _ = TranslateMessage(&message); DispatchMessageW(&message);
        }
        let result = match session.outcome.take() {
            Some(Outcome::Failed(error)) => { drop(session); return Err(error.into()); }
            Some(Outcome::Selected {monitor, rect, confirmed_at}) => {
                let rgba = session.frames[monitor].image.crop(rect)?;
                Some(SelectedImage {width:rect.width() as u32, height:rect.height() as u32, rgba, confirmed_at})
            }
            _ => None,
        };
        // 必须先销毁窗口，再释放其 USERDATA。冻结帧也在此释放。
        drop(session);
        drop(window_data);
        Ok(result)
    }
}

#[cfg(test)]
#[path = "capture_render_tests.rs"]
mod render_tests;

#[cfg(test)]
#[path = "capture_interaction_tests.rs"]
mod interaction_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_uses_physical_pixels_in_all_drag_directions() {
        let expected = PixelRect {left:100, top:200, right:500, bottom:500};
        for (a, b) in [((100,200),(500,500)), ((500,500),(100,200)), ((500,200),(100,500)), ((100,500),(500,200))] {
            assert_eq!(selection_rect(a,b,3840,2160), expected);
        }
        assert_eq!(expected.width(),400); assert_eq!(expected.height(),300);
    }
    #[test]
    fn captured_mouse_clamps_to_locked_display_edges() {
        assert_eq!(selection_rect((100,200),(-3840,3000),3840,2160), PixelRect {left:0,top:200,right:100,bottom:2160});
        // 左侧显示器的 (-3440, 300) 转为本屏 (400, 300)，不乘 200% 缩放。
        assert_eq!(screen_to_local((-3440,300),PixelRect{left:-3840,top:0,right:0,bottom:2160}),(400,300));
        assert_eq!(screen_to_local((400,-500),PixelRect{left:-3840,top:0,right:0,bottom:2160}),(4240,-500));
        assert_eq!(selection_rect((0,0),(400,300),3840,2160).width(),400);
    }
    #[test]
    fn crop_changes_bgra_to_opaque_rgba_without_row_or_origin_errors() {
        let source: Vec<u8> = (0..12).flat_map(|i| [i, i+20, i+40, 0]).collect();
        let rect = PixelRect {left:1,top:1,right:3,bottom:3};
        let result = crop_bgra(&source,4,rect,16).unwrap();
        assert_eq!(result, vec![45,25,5,255,46,26,6,255,49,29,9,255,50,30,10,255]);
        assert!(crop_bgra(&source[..4],4,rect,16).is_err());
    }
    #[test]
    fn invalid_or_overflowing_sizes_are_rejected() {
        assert!(byte_length(0,300).is_err()); assert!(byte_length(400,-1).is_err());
        assert!(byte_length(i32::MAX,i32::MAX).is_err());
        assert_eq!(byte_length(3840,2160).unwrap(),33_177_600);
    }

    /// 只测真实显示器到内存的取帧；不创建遮罩、不改运行中岛的属性、不保存屏幕内容。
    #[test]
    #[ignore = "需要显式运行 release；仅内存取帧并立即释放"]
    fn stage_a_release_frame_benchmark() {
        assert!(!cfg!(debug_assertions), "请使用 cargo test --release 运行性能测试");
        let _dpi = DpiGuard::new().unwrap();
        unsafe {
            let displays = monitors().unwrap();
            let desktop = DesktopDc(GetDC(None));
            assert!(!desktop.0.is_invalid());
            for sample in 1..=5 {
                DwmFlush().unwrap();
                for (index, bounds) in displays.iter().copied().enumerate() {
                    let (frame, capture_ms, bitblt_ms) = capture_frame(desktop.0, bounds).unwrap();
                    // 立即删除 DIB；日志只含尺寸/坐标/时间，不含任何像素或文字内容。
                    drop(frame);
                    timing(&format!("bench_frame memory_only no_affinity sample={sample} monitor={} origin=({}, {}) size={}x{} capture_ms={capture_ms:.3} bitblt_ms={bitblt_ms:.3}", index + 1,bounds.left,bounds.top,bounds.width(),bounds.height()));
                }
            }
        }
    }
}
