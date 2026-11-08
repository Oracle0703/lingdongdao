//! 截图标注（设计文档第 12 节）：标注数据、几何、马赛克与 GDI+ 抗锯齿绘制。
//!
//! 坐标都是所在屏幕的局部物理像素。线条、箭头和文字用 GDI+ 画（GDI 画线有锯齿）；
//! 马赛克直接对 BGRA 像素按块取平均，方块网格按屏幕坐标对齐，预览与保存结果一致。

use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus as gdip;

use super::selection::{PixelRect, Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool { Box, Arrow, Pen, Text, Mosaic }

impl Tool {
    pub(super) const ALL: [Tool; 5] = [Tool::Box, Tool::Arrow, Tool::Pen, Tool::Text, Tool::Mosaic];
    pub(super) fn label(self) -> &'static str {
        match self { Tool::Box => "框", Tool::Arrow => "箭头", Tool::Pen => "画笔", Tool::Text => "文字", Tool::Mosaic => "打码" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Box(Point, Point),
    Arrow(Point, Point),
    Pen(Vec<Point>),
    Mosaic(Point, Point),
    /// `size`：完成时测得的文字宽高，用于计算重绘范围。
    Text { at: Point, text: String, size: (i32, i32) },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mark { pub monitor: usize, pub shape: Shape }

/// 标注样式，已按所在屏幕的 DPI 换算成物理像素。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Style { pub line: f32, pub head: f32, pub font: f32, pub block: i32, pub min_stroke: i32 }

impl Style {
    pub(super) fn for_dpi(dpi: u32) -> Self {
        let k = dpi.max(96) as f32 / 96.0;
        Self { line: (3.0 * k).round(), head: (14.0 * k).round(), font: (18.0 * k).round(), block: (10.0 * k).round().max(2.0) as i32, min_stroke: (3.0 * k).round() as i32 }
    }
}

/// `#FF3B30`，ARGB（GDI+）。
pub(super) const RED: u32 = 0xFFFF_3B30;

fn normalized(a: Point, b: Point) -> PixelRect {
    PixelRect { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

/// 太短的一笔（误点）不留痕；文字为空也不留。
pub(super) fn too_short(shape: &Shape, style: Style) -> bool {
    let far = |a: Point, b: Point| (a.0 - b.0).abs().max((a.1 - b.1).abs()) >= style.min_stroke;
    match shape {
        Shape::Box(a, b) | Shape::Mosaic(a, b) => { let r = normalized(*a, *b); r.width() < style.min_stroke || r.height() < style.min_stroke }
        Shape::Arrow(a, b) => !far(*a, *b),
        Shape::Pen(points) => points.len() < 2 || !points.iter().any(|p| far(points[0], *p)),
        Shape::Text { text, .. } => text.trim().is_empty(),
    }
}

/// 箭头头部三角形：（尖端，左翼，右翼）。头部长度不超过箭杆长度的一半，避免短箭头变成一团。
pub(super) fn arrow_head(from: Point, to: Point, style: Style) -> Option<[(f32, f32); 3]> {
    let (dx, dy) = ((to.0 - from.0) as f32, (to.1 - from.1) as f32);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 { return None; }
    let (ux, uy) = (dx / length, dy / length);
    let head = style.head.min(length * 0.5).max(style.line * 2.0);
    let half = head * 0.45;
    let base = (to.0 as f32 - ux * head, to.1 as f32 - uy * head);
    Some([(to.0 as f32, to.1 as f32), (base.0 - uy * half, base.1 + ux * half), (base.0 + uy * half, base.1 - ux * half)])
}

/// 这一笔会画到的范围（含线宽和箭头头部），用于局部重绘。
pub(super) fn bounds(shape: &Shape, style: Style) -> PixelRect {
    let pad = (style.line / 2.0).ceil() as i32 + 2;
    match shape {
        Shape::Box(a, b) => normalized(*a, *b).expand(pad),
        Shape::Mosaic(a, b) => normalized(*a, *b).expand(style.block),
        Shape::Arrow(a, b) => normalized(*a, *b).expand(pad + style.head.ceil() as i32),
        Shape::Pen(points) => points.iter().fold(PixelRect { left: i32::MAX, top: i32::MAX, right: i32::MIN, bottom: i32::MIN },
            |r, p| PixelRect { left: r.left.min(p.0), top: r.top.min(p.1), right: r.right.max(p.0), bottom: r.bottom.max(p.1) }).expand(pad),
        Shape::Text { at, size, .. } => PixelRect { left: at.0, top: at.1, right: at.0 + size.0, bottom: at.1 + size.1 }.expand(pad + 2),
    }
}

/// 在完整帧（BGRA、自上而下、无行填充）上原地打马赛克。方块按屏幕坐标对齐，块内取平均色。
pub(super) fn mosaic_in_place(bits: &mut [u8], width: i32, height: i32, area: PixelRect, block: i32) {
    let frame = PixelRect { left: 0, top: 0, right: width, bottom: height };
    let Some(area) = area.intersect(frame) else { return };
    for (cell, color) in cells(bits, width, area, block) {
        fill_cell(bits, width, frame, cell, color);
    }
}

/// 拖动中的预览：平均色取自完整帧 `source`，写入覆盖 `target_rect`（屏幕坐标）的小缓冲 `target`。
pub(super) fn mosaic_into(source: &[u8], width: i32, height: i32, target: &mut [u8], target_rect: PixelRect, area: PixelRect, block: i32) {
    let frame = PixelRect { left: 0, top: 0, right: width, bottom: height };
    let Some(area) = area.intersect(frame) else { return };
    for (cell, color) in cells(source, width, area, block) {
        if let Some(visible) = cell.intersect(target_rect) {
            fill_cell(target, target_rect.width(), target_rect, visible, color);
        }
    }
}

fn cells(bits: &[u8], width: i32, area: PixelRect, block: i32) -> Vec<(PixelRect, [u8; 4])> {
    let block = block.max(1);
    let mut out = Vec::new();
    let mut y = area.top.div_euclid(block) * block;
    while y < area.bottom {
        let mut x = area.left.div_euclid(block) * block;
        while x < area.right {
            let cell = PixelRect { left: x, top: y, right: x + block, bottom: y + block };
            if let Some(cell) = cell.intersect(area) {
                let mut sum = [0u64; 4];
                for py in cell.top..cell.bottom {
                    let row = (py as usize * width as usize) * 4;
                    for px in cell.left..cell.right {
                        let i = row + px as usize * 4;
                        if let Some(pixel) = bits.get(i..i + 4) { for c in 0..4 { sum[c] += pixel[c] as u64; } }
                    }
                }
                let n = (cell.width() as u64 * cell.height() as u64).max(1);
                out.push((cell, [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]));
            }
            x += block;
        }
        y += block;
    }
    out
}

fn fill_cell(bits: &mut [u8], stride_pixels: i32, origin: PixelRect, cell: PixelRect, color: [u8; 4]) {
    for py in cell.top..cell.bottom {
        let row = ((py - origin.top) as usize * stride_pixels as usize) * 4;
        for px in cell.left..cell.right {
            let i = row + (px - origin.left) as usize * 4;
            if let Some(pixel) = bits.get_mut(i..i + 4) { pixel.copy_from_slice(&color); }
        }
    }
}

fn started() -> bool {
    static TOKEN: OnceLock<Option<usize>> = OnceLock::new();
    TOKEN.get_or_init(|| unsafe {
        let mut token = 0usize;
        let input = gdip::GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        (gdip::GdiplusStartup(&mut token, &input, std::ptr::null_mut()).0 == 0).then_some(token)
    }).is_some()
}

fn check(status: gdip::Status, what: &str) -> Result<(), String> {
    if status.0 == 0 { Ok(()) } else { Err(format!("{what}失败（GDI+ 状态 {}）", status.0)) }
}

/// GDI+ 绘图对象，离开作用域时释放。
struct Graphics(*mut gdip::GpGraphics);
impl Graphics {
    unsafe fn new(dc: HDC) -> Result<Self, String> {
        if !started() { return Err("GDI+ 启动失败".into()); }
        let mut graphics = std::ptr::null_mut();
        check(gdip::GdipCreateFromHDC(dc, &mut graphics), "创建标注画布")?;
        let graphics = Self(graphics);
        check(gdip::GdipSetSmoothingMode(graphics.0, gdip::SmoothingModeAntiAlias), "设置抗锯齿")?;
        check(gdip::GdipSetTextRenderingHint(graphics.0, gdip::TextRenderingHintAntiAlias), "设置文字抗锯齿")?;
        Ok(graphics)
    }
}
impl Drop for Graphics { fn drop(&mut self) { unsafe { gdip::GdipDeleteGraphics(self.0); } } }

struct Pen(*mut gdip::GpPen);
impl Drop for Pen { fn drop(&mut self) { unsafe { gdip::GdipDeletePen(self.0); } } }
struct Brush(*mut gdip::GpSolidFill);
impl Drop for Brush { fn drop(&mut self) { unsafe { gdip::GdipDeleteBrush(self.0 as *mut gdip::GpBrush); } } }
struct Family(*mut gdip::GpFontFamily);
impl Drop for Family { fn drop(&mut self) { unsafe { gdip::GdipDeleteFontFamily(self.0); } } }
struct Font(*mut gdip::GpFont);
impl Drop for Font { fn drop(&mut self) { unsafe { gdip::GdipDeleteFont(self.0); } } }

unsafe fn pen(style: Style) -> Result<Pen, String> {
    let mut pen = std::ptr::null_mut();
    check(gdip::GdipCreatePen1(RED, style.line, gdip::UnitPixel, &mut pen), "创建画笔")?;
    let pen = Pen(pen);
    check(gdip::GdipSetPenLineJoin(pen.0, gdip::LineJoinRound), "设置线条拐角")?;
    check(gdip::GdipSetPenStartCap(pen.0, gdip::LineCapRound), "设置线头")?;
    check(gdip::GdipSetPenEndCap(pen.0, gdip::LineCapRound), "设置线尾")?;
    Ok(pen)
}

unsafe fn brush() -> Result<Brush, String> {
    let mut brush = std::ptr::null_mut();
    check(gdip::GdipCreateSolidFill(RED, &mut brush), "创建填充色")?;
    Ok(Brush(brush))
}

unsafe fn font(style: Style) -> Result<(Family, Font), String> {
    let mut family = std::ptr::null_mut();
    check(gdip::GdipCreateFontFamilyFromName(w!("Microsoft YaHei"), std::ptr::null_mut(), &mut family), "加载微软雅黑")?;
    let family = Family(family);
    let mut font = std::ptr::null_mut();
    // FontStyleBold = 1
    check(gdip::GdipCreateFont(family.0, style.font, 1, gdip::UnitPixel, &mut font), "创建标注字体")?;
    Ok((family, Font(font)))
}

/// 测量一段标注文字的宽高（物理像素）。
pub(super) unsafe fn measure_text(dc: HDC, text: &str, style: Style) -> Result<(i32, i32), String> {
    let graphics = Graphics::new(dc)?;
    let (_family, font) = font(style)?;
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = gdip::RectF { X: 0.0, Y: 0.0, Width: 100_000.0, Height: 100_000.0 };
    let mut bounds = gdip::RectF::default();
    check(gdip::GdipMeasureString(graphics.0, windows::core::PCWSTR(wide.as_ptr()), wide.len() as i32, font.0, &layout, std::ptr::null(), &mut bounds, std::ptr::null_mut(), std::ptr::null_mut()), "测量标注文字")?;
    Ok((bounds.Width.ceil() as i32, bounds.Height.ceil() as i32))
}

/// 用 GDI+ 把一笔画到 `dc` 上；`offset` 为 `dc` 原点对应的屏幕坐标（局部缓冲时不为 0）。马赛克不在这里画。
pub(super) unsafe fn draw(dc: HDC, shape: &Shape, style: Style, offset: Point) -> Result<(), String> {
    let at = |p: Point| gdip::Point { X: p.0 - offset.0, Y: p.1 - offset.1 };
    let graphics = Graphics::new(dc)?;
    match shape {
        Shape::Box(a, b) => {
            let r = normalized(*a, *b);
            let pen = pen(style)?;
            check(gdip::GdipDrawRectangleI(graphics.0, pen.0, r.left - offset.0, r.top - offset.1, r.width(), r.height()), "画框")?;
        }
        Shape::Arrow(from, to) => {
            let Some(head) = arrow_head(*from, *to, style) else { return Ok(()) };
            // 箭杆画到头部底边，圆头不会从尖端露出。
            let base = (((head[1].0 + head[2].0) / 2.0).round() as i32, ((head[1].1 + head[2].1) / 2.0).round() as i32);
            let pen = pen(style)?;
            let shaft = [at(*from), at(base)];
            check(gdip::GdipDrawLinesI(graphics.0, pen.0, shaft.as_ptr(), 2), "画箭杆")?;
            let brush = brush()?;
            let tri: Vec<_> = head.iter().map(|p| at((p.0.round() as i32, p.1.round() as i32))).collect();
            check(gdip::GdipFillPolygonI(graphics.0, brush.0 as *mut gdip::GpBrush, tri.as_ptr(), 3, gdip::FillModeAlternate), "画箭头")?;
        }
        Shape::Pen(points) if points.len() >= 2 => {
            let pen = pen(style)?;
            let line: Vec<_> = points.iter().map(|p| at(*p)).collect();
            check(gdip::GdipDrawLinesI(graphics.0, pen.0, line.as_ptr(), line.len() as i32), "画笔迹")?;
        }
        Shape::Text { at: origin, text, .. } => {
            let (_family, font) = font(style)?;
            let brush = brush()?;
            let wide: Vec<u16> = text.encode_utf16().collect();
            let layout = gdip::RectF { X: (origin.0 - offset.0) as f32, Y: (origin.1 - offset.1) as f32, Width: 100_000.0, Height: 100_000.0 };
            check(gdip::GdipDrawString(graphics.0, windows::core::PCWSTR(wide.as_ptr()), wide.len() as i32, font.0, &layout, std::ptr::null(), brush.0 as *const gdip::GpBrush), "写标注文字")?;
        }
        Shape::Pen(_) | Shape::Mosaic(..) => {}
    }
    Ok(())
}

/// 已标注帧：原帧像素 + 依次叠加的标注（马赛克是像素运算，其余用 GDI+）。
/// `bits` 与 `dc` 指向同一块 DIB；先 GdiFlush，保证 GDI+ 的绘制写进像素后再做马赛克。
pub(super) unsafe fn render(dc: HDC, bits: &mut [u8], width: i32, height: i32, marks: &[&Shape], style: Style) -> Result<(), String> {
    for shape in marks {
        match shape {
            Shape::Mosaic(a, b) => {
                let _ = windows::Win32::Graphics::Gdi::GdiFlush();
                mosaic_in_place(bits, width, height, normalized(*a, *b), style.block);
            }
            other => draw(dc, other, style, (0, 0))?,
        }
    }
    let _ = windows::Win32::Graphics::Gdi::GdiFlush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style { Style::for_dpi(96) }

    #[test]
    fn style_scales_with_dpi() {
        let normal = Style::for_dpi(96);
        let double = Style::for_dpi(192);
        assert_eq!((normal.line, normal.font, normal.block), (3.0, 18.0, 10));
        assert_eq!((double.line, double.font, double.block), (6.0, 36.0, 20));
        assert_eq!(Style::for_dpi(144).line, 5.0);
    }

    #[test]
    fn tiny_strokes_and_empty_text_leave_no_mark() {
        let s = style();
        assert!(too_short(&Shape::Box((10, 10), (11, 40)), s), "太窄的框");
        assert!(!too_short(&Shape::Box((10, 10), (20, 40)), s));
        assert!(too_short(&Shape::Arrow((10, 10), (11, 11)), s));
        assert!(!too_short(&Shape::Arrow((10, 10), (10, 30)), s));
        assert!(too_short(&Shape::Pen(vec![(5, 5)]), s));
        assert!(too_short(&Shape::Pen(vec![(5, 5), (6, 6), (5, 6)]), s));
        assert!(!too_short(&Shape::Pen(vec![(5, 5), (6, 6), (20, 9)]), s));
        assert!(too_short(&Shape::Text { at: (0, 0), text: "  ".into(), size: (0, 0) }, s));
        assert!(!too_short(&Shape::Text { at: (0, 0), text: "注意".into(), size: (30, 20) }, s));
    }

    #[test]
    fn arrow_head_points_at_the_end_and_shrinks_on_short_arrows() {
        let s = style();
        let [tip, left, right] = arrow_head((0, 0), (100, 0), s).unwrap();
        assert_eq!(tip, (100.0, 0.0));
        assert!((left.0 - 86.0).abs() < 0.01 && (right.0 - 86.0).abs() < 0.01, "头部长 14");
        assert!(left.1 > 0.0 && right.1 < 0.0 && (left.1 + right.1).abs() < 0.01, "两翼对称");
        let [_, short_left, _] = arrow_head((0, 0), (10, 0), s).unwrap();
        assert!(short_left.0 >= 4.0, "短箭头的头部不超过一半长度（线宽两倍以下除外）");
        assert!(arrow_head((5, 5), (5, 5), s).is_none());
    }

    #[test]
    fn bounds_cover_line_width_and_arrow_head() {
        let s = Style::for_dpi(192);
        let b = bounds(&Shape::Box((100, 100), (50, 80)), s);
        assert!(b.left < 50 && b.top < 80 && b.right > 100 && b.bottom > 100);
        let a = bounds(&Shape::Arrow((0, 0), (200, 0)), s);
        assert!(a.top <= -(s.head as i32) && a.bottom >= s.head as i32, "箭头头部可能伸出箭杆所在的直线");
        let p = bounds(&Shape::Pen(vec![(10, 10), (40, 5), (20, 60)]), s);
        assert!(p.left < 10 && p.top < 5 && p.right > 40 && p.bottom > 60);
    }

    #[test]
    fn mosaic_averages_blocks_aligned_to_the_screen_grid() {
        // 20 × 10 的帧：左半黑、右半白。
        let (w, h) = (20, 10);
        let mut bits: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| if x < 10 { [0u8, 0, 0, 255] } else { [200, 200, 200, 255] })).collect();
        mosaic_in_place(&mut bits, w, h, PixelRect { left: 5, top: 0, right: 15, bottom: 10 }, 10);
        let px = |x: usize, y: usize| { let i = (y * w as usize + x) * 4; [bits[i], bits[i + 1], bits[i + 2]] };
        assert_eq!(px(0, 0), [0, 0, 0], "区域外不变");
        assert_eq!(px(19, 9), [200, 200, 200], "区域外不变");
        assert_eq!(px(5, 3), [0, 0, 0], "块按 10 对齐：5..10 落在第一块（全黑）");
        assert_eq!(px(12, 3), [200, 200, 200], "10..15 落在第二块（全白）");
        // 预览写进局部缓冲，结果与原地处理一致。
        let source: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 10) as u8, 0, 0, 255])).collect();
        let mut full = source.clone();
        mosaic_in_place(&mut full, w, h, PixelRect { left: 2, top: 2, right: 18, bottom: 8 }, 4);
        let target_rect = PixelRect { left: 6, top: 3, right: 14, bottom: 7 };
        let mut target = vec![0u8; (target_rect.width() * target_rect.height() * 4) as usize];
        mosaic_into(&source, w, h, &mut target, target_rect, PixelRect { left: 2, top: 2, right: 18, bottom: 8 }, 4);
        for y in target_rect.top..target_rect.bottom {
            for x in target_rect.left..target_rect.right {
                let a = ((y * w + x) * 4) as usize;
                let b = (((y - target_rect.top) * target_rect.width() + (x - target_rect.left)) * 4) as usize;
                assert_eq!(full[a..a + 3], target[b..b + 3], "({x},{y})");
            }
        }
    }
}
