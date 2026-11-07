//! 使用合成 DIB 和屏幕外、不激活的窗口验证原生绘制，不读取用户桌面。
use super::*;

pub(super) fn synthetic_frame(width: i32, height: i32) -> Frame {
    let image = unsafe { Bitmap::new(HDC::default(), width, height).unwrap() };
    let bytes = unsafe { std::slice::from_raw_parts_mut(image.bits, byte_length(width,height).unwrap()) };
    for (index, pixel) in bytes.chunks_exact_mut(4).enumerate() {
        let x = index % width as usize;
        let y = index / width as usize;
        pixel.copy_from_slice(&[(x%256) as u8,(y%256) as u8,((x/8)^(y/8)) as u8,255]);
    }
    Frame {bounds:PixelRect {left:0,top:0,right:width,bottom:height},image}
}

fn visual(monitor: usize, start: (i32,i32), end: (i32,i32), adjustable: bool) -> VisualSelection {
    VisualSelection {selection:Selection {monitor,rect:PixelRect {left:start.0.min(end.0),top:start.1.min(end.1),right:start.0.max(end.0),bottom:start.1.max(end.1)},adjustable},pressed:None,tool:None,undo:false}
}

fn pixels(image: &Bitmap) -> Vec<u8> {
    unsafe {
        assert!(GdiFlush().as_bool());
        std::slice::from_raw_parts(image.bits,byte_length(image.width,image.height).unwrap()).to_vec()
    }
}

fn contains(outer: PixelRect, inner: PixelRect) -> bool {
    outer.left <= inner.left && outer.top <= inner.top && outer.right >= inner.right && outer.bottom >= inner.bottom
}

fn assert_same_pixels(actual: &[u8], expected: &[u8], context: &str) {
    assert_eq!(actual.len(),expected.len(),"{context}：像素长度不同");
    let mismatch = actual.iter().zip(expected).position(|(a,b)|a!=b);
    assert!(mismatch.is_none(),"{context}：第一个不一致的像素字节索引 {mismatch:?}");
}

#[test]
fn dirty_region_covers_old_new_selection_border_labels_and_hint_transitions() {
    let frame = synthetic_frame(1200,800);
    let style = OverlayStyle::new(192).unwrap();
    let first = Some(visual(1,(450,400),(650,600),true));
    let reversed = Some(visual(1,(450,400),(50,4),true));
    for (before,after) in [(None,first),(first,reversed),(reversed,None)] {
        let old = overlay_layout(&frame,&style,before).unwrap();
        let new = overlay_layout(&frame,&style,after).unwrap();
        let dirty = dirty_region(&old,&new,style.metrics,frame.bounds).unwrap();
        for label in [old.label,new.label] { assert!(contains(dirty,label)); }
        for rect in [old.selection,new.selection].into_iter().flatten() {
            let expanded = rect.expand(style.metrics.selection.handle_size/2+style.metrics.border).intersect(frame.bounds).unwrap();
            assert!(contains(dirty,expanded));
        }
        for toolbar in [old.toolbar,new.toolbar].into_iter().flatten() {assert!(contains(dirty,toolbar.bounds));}
        for handle in old.handles.iter().chain(&new.handles) {if let Some(rect)=handle.intersect(frame.bounds) {assert!(contains(dirty,rect));}}
        assert_eq!(changed_monitors(before,after),vec![1]);
    }
    assert!(changed_monitors(first,first).is_empty());
    // 在真实的 4K、200% 帧上，选区挪 10 像素只重画周围一小块（工具栏较宽，也远小于整屏）。
    let large = synthetic_frame(3840,2160);
    let stationary = overlay_layout(&large,&style,Some(visual(0,(1400,900),(1450,950),true))).unwrap();
    let moved = overlay_layout(&large,&style,Some(visual(0,(1400,900),(1460,960),true))).unwrap();
    let dirty = dirty_region(&stationary,&moved,style.metrics,large.bounds).unwrap();
    assert!(dirty.width()*dirty.height() < large.image.width*large.image.height/8);
}

#[test]
fn dpi_200_scales_font_and_label_geometry_but_not_selection_pixels() {
    let frame = synthetic_frame(1200,800);
    let normal = OverlayStyle::new(96).unwrap();
    let doubled = OverlayStyle::new(192).unwrap();
    assert_eq!(doubled.metrics.font_height,normal.metrics.font_height*2);
    assert_eq!(doubled.metrics.margin,normal.metrics.margin*2);
    assert_eq!(doubled.metrics.border,normal.metrics.border*2);
    unsafe {
        let mut font = LOGFONTW::default();
        assert_eq!(GetObjectW(doubled.font.0,std::mem::size_of::<LOGFONTW>() as i32,Some((&mut font as *mut LOGFONTW).cast())),std::mem::size_of::<LOGFONTW>() as i32);
        assert_eq!(font.lfHeight,-32);
        assert!(String::from_utf16_lossy(&font.lfFaceName).starts_with("Microsoft YaHei"));
    }
    for state in [None,Some(visual(0,(790,490),(1190,790),true)),Some(visual(0,(1190,2),(1198,6),true))] {
        let small = overlay_layout(&frame,&normal,state).unwrap();
        let large = overlay_layout(&frame,&doubled,state).unwrap();
        assert_eq!(small.selection,large.selection);
        assert!(contains(frame.bounds,large.label));
        let text = doubled.measure(frame.image.dc,&large.text).unwrap();
        assert_eq!(large.label.width(),text.cx+2*doubled.metrics.padding_x);
        assert_eq!(large.label.height(),text.cy+2*doubled.metrics.padding_y);
        assert!(large.label.width() > small.label.width()*17/10);
        if state.is_some() {
            assert_eq!(large.handles.len(),8);
            assert!(large.handles.iter().all(|handle|handle.width()==16 && handle.height()==16));
            let toolbar=large.toolbar.unwrap();
            assert!(contains(frame.bounds,toolbar.bounds));
            assert_eq!(toolbar.save.height(),small.toolbar.unwrap().save.height()*2);
        }
    }
}

#[test]
fn buffered_dirty_updates_match_full_redraw_and_keep_original_pixels() {
    let frame = synthetic_frame(960,600);
    let original = pixels(&frame.image);
    let dark = frame.image.darkened().unwrap();
    let dark_before = pixels(&dark);
    assert_ne!(dark_before,original);
    let style = OverlayStyle::new(192).unwrap();
    let idle = overlay_layout(&frame,&style,None).unwrap();
    let display = unsafe { render_dirty(&frame,&dark,&style,&idle,frame.bounds).unwrap() };
    let states = [
        Some(visual(0,(480,300),(480,300),false)),
        Some(visual(0,(480,300),(720,520),false)),
        Some(visual(0,(480,300),(720,520),true)),
        Some(visual(0,(300,180),(540,400),true)),
        Some(visual(0,(480,300),(90,5),true)),
        Some(visual(0,(480,300),(479,301),true)),
        None,
        Some(visual(0,(950,590),(959,599),true)),
        Some(visual(0,(0,0),(960,600),true)),
        Some(VisualSelection {pressed:Some(Button::Save),..visual(0,(0,0),(960,600),true)}),
        None,
    ];
    let mut before = None;
    for after in states {
        let old = overlay_layout(&frame,&style,before).unwrap();
        let new = overlay_layout(&frame,&style,after).unwrap();
        let dirty = dirty_region(&old,&new,style.metrics,frame.bounds).unwrap();
        unsafe {
            let patch = render_dirty(&frame,&dark,&style,&new,dirty).unwrap();
            assert_eq!((patch.width,patch.height),(dirty.width(),dirty.height()));
            BitBlt(display.dc,dirty.left,dirty.top,dirty.width(),dirty.height(),patch.dc,0,0,SRCCOPY).unwrap();
            let expected = render_dirty(&frame,&dark,&style,&new,frame.bounds).unwrap();
            assert_same_pixels(&pixels(&display),&pixels(&expected),&format!("局部重绘必须清除旧边框/旧文字：{before:?} -> {after:?}"));
        }
        before = after;
    }
    assert_same_pixels(&pixels(&frame.image),&original,"暗帧、选区和文字绘制不可改动 PNG 的原始像素");
    assert_same_pixels(&pixels(&dark),&dark_before,"暗帧应在会话期间保持只读");
}

#[test]
fn drag_invalidates_only_its_offscreen_monitor_window() {
    let cancel = AtomicBool::new(false);
    let frames = vec![synthetic_frame(640,480),synthetic_frame(640,480)];
    let dark_frames = frames.iter().map(|frame|frame.image.darkened().unwrap()).collect();
    let session = Session {frames,dark_frames,styles:RefCell::new(Vec::new()),windows:RefCell::new(Vec::new()),interaction:Cell::new(Interaction::default()),outcome:Cell::new(None),armed:Cell::new(false),cancel:&cancel,previous_foreground:HWND::default(),marks:RefCell::new(Vec::new()),annotated:RefCell::new(Vec::new()),stroke:RefCell::new(Vec::new()),edit:RefCell::new(None),anno_styles:RefCell::new(Vec::new())};
    unsafe {
        let module = GetModuleHandleW(None).unwrap();
        for index in 0..2 {
            // Windows 会丢弃隐藏窗口的更新区，必须用可绘制的屏幕外窗口检查实际失效。
            // 整窗位于虚拟桌面左边，NOACTIVATE 防止激活；两个测试窗口也互不覆盖。
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN)-1024*(index+1);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let hwnd = CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,w!("STATIC"),w!("截图绘制回归"),WS_POPUP|WS_VISIBLE,x,y,640,480,None,None,HINSTANCE(module.0),None).unwrap();
            session.windows.borrow_mut().push(hwnd);
            session.styles.borrow_mut().push(OverlayStyle::new(GetDpiForWindow(hwnd)).unwrap());
        }
        let windows = session.windows.borrow().clone();
        for &hwnd in &windows {
            assert!(IsWindowVisible(hwnd).as_bool());
            assert!(ValidateRect(hwnd,None).as_bool());
        }
        session.pointer_down(1,(350,260));session.pointer_move(1,(450,350));
        assert!(!GetUpdateRect(windows[0],None,false).as_bool(),"非拖动屏不得因鼠标移动失效");
        let mut dirty = RECT::default();
        assert!(GetUpdateRect(windows[1],Some(&mut dirty),false).as_bool());
        assert!(dirty.right > dirty.left && dirty.bottom > dirty.top);
        for &hwnd in &windows { assert!(ValidateRect(hwnd,None).as_bool()); }
        session.pointer_move(1,(460,360));
        assert!(!GetUpdateRect(windows[0],None,false).as_bool());
        assert!(GetUpdateRect(windows[1],Some(&mut dirty),false).as_bool());
        assert!(dirty.right-dirty.left < 640 && dirty.bottom-dirty.top < 480);
        session.pointer_up(1,(460,360));
        assert!(session.outcome.get().is_none(),"松开只显示调整控件，不能开始保存");
        for &hwnd in &windows {assert!(ValidateRect(hwnd,None).as_bool());}
        session.pointer_down(0,(40,40));
        assert!(!GetUpdateRect(windows[1],None,false).as_bool(),"只按下不拖动时保留旧选区，不重画");
        session.pointer_move(0,(140,140));
        assert!(GetUpdateRect(windows[0],None,false).as_bool(),"新屏需要绘制新选区");
        assert!(GetUpdateRect(windows[1],None,false).as_bool(),"旧屏的控制点与工具栏必须清除");
    }
}

#[test]
fn adjusted_labels_avoid_toolbar_and_handles_with_margin_at_screen_edges() {
    let frame=synthetic_frame(1200,800);
    for dpi in [96,120,144,192] {
        let style=OverlayStyle::new(dpi).unwrap();
        for (start,end) in [((220,160),(920,560)),((300,520),(950,790)),((0,0),(1200,800)),((580,360),(581,361)),((500,2),(650,120)),
            ((1,1),(40,50)),((1160,1),(1199,50)),((1,760),(40,799)),((1160,760),(1199,799)),((590,0),(610,800)),((0,390),(1200,410))] {
            let layout=overlay_layout(&frame,&style,Some(visual(0,start,end,true))).unwrap();
            let toolbar=layout.toolbar.unwrap();
            assert_eq!(toolbar,selection::toolbar_layout(layout.selection.unwrap(),frame.bounds,style.metrics.selection),"不能为了放标签改变工具栏位置");
            assert!(layout.label.intersect(toolbar.bounds.expand(style.metrics.gap)).is_none(),"{dpi} DPI 下标签须避让工具栏及间距：{start:?} -> {end:?}");
            for handle in &layout.handles {
                assert!(layout.label.intersect(handle.expand(style.metrics.gap)).is_none(),"{dpi} DPI 下标签须避让控制点及间距：{start:?} -> {end:?}");
            }
            assert!(contains(frame.bounds,layout.label) && contains(frame.bounds,toolbar.bounds));
        }
    }
}

/// 只导出合成像素，复用产品真实绘制；不显示原生窗口、不采集用户桌面。
#[test]
#[ignore = "按需导出 v0.4 确认/调整遮罩的 150% 和 200% 合成布局图"]
fn v04_overlay_gallery() {
    let directory=std::path::Path::new(r"E:\tmp\lingdongdao-notes\stage-a-v04\overlay-gallery");
    std::fs::create_dir_all(directory).unwrap();
    let frame=synthetic_frame(1200,800);let dark=frame.image.darkened().unwrap();
    for (dpi,scale) in [(144,150),(192,200)] {
        let style=OverlayStyle::new(dpi).unwrap();
        for (name,start,end) in [("adjusting",(220,160),(920,560)),("bottom",(300,520),(950,790)),
            ("full",(0,0),(1200,800)),("tiny-disabled",(580,360),(581,361)),("top-narrow",(500,2),(650,120))] {
            let layout=overlay_layout(&frame,&style,Some(visual(0,start,end,true))).unwrap();
            assert!(layout.label.intersect(layout.toolbar.unwrap().bounds.expand(style.metrics.gap)).is_none());
            assert!(layout.handles.iter().all(|handle|layout.label.intersect(handle.expand(style.metrics.gap)).is_none()));
            let painted=unsafe {render_dirty(&frame,&dark,&style,&layout,frame.bounds).unwrap()};
            let rgba=painted.crop(frame.bounds).unwrap();
            let path=directory.join(format!("{scale}-{name}.png"));
            image::save_buffer_with_format(&path,&rgba,1200,800,image::ColorType::Rgba8,image::ImageFormat::Png).unwrap();
            println!("{}",path.display());
        }
    }
}
