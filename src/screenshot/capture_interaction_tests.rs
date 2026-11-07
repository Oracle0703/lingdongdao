//! 驱动生产 Session 的事件入口；所有像素为合成数据，窗口从不显示或取得鼠标捕获。
use super::*;

fn session(cancel: &AtomicBool) -> Session<'_> {
    let mut frames=vec![super::render_tests::synthetic_frame(1200,800),super::render_tests::synthetic_frame(1200,800)];
    frames[1].bounds=PixelRect {left:-1200,top:0,right:0,bottom:800};
    let dark_frames=frames.iter().map(|frame|frame.image.darkened().unwrap()).collect();
    let session=Session {frames,dark_frames,styles:RefCell::new(Vec::new()),windows:RefCell::new(Vec::new()),interaction:Cell::new(Interaction::default()),outcome:Cell::new(None),armed:Cell::new(false),cancel,previous_foreground:HWND::default(),marks:RefCell::new(Vec::new()),annotated:RefCell::new(Vec::new()),stroke:RefCell::new(Vec::new()),edit:RefCell::new(None),anno_styles:RefCell::new(Vec::new())};
    unsafe {
        for dpi in [96,192] {
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,w!("STATIC"),w!("截图交互回归"),WS_POPUP,0,0,1200,800,None,None,None,None).unwrap();
            session.windows.borrow_mut().push(hwnd);
            session.styles.borrow_mut().push(OverlayStyle::new(dpi).unwrap());
            session.anno_styles.borrow_mut().push(annotate::Style::for_dpi(dpi));
        }
    }
    session
}

fn draw(session: &Session<'_>, monitor: usize, start: (i32,i32), end: (i32,i32)) {
    assert!(session.pointer_down(monitor,start));
    session.pointer_move(monitor,end);
    session.pointer_up(monitor,end);
    assert!(session.interaction.get().gesture.is_none());
    assert!(session.interaction.get().selection.unwrap().adjustable);
    assert!(session.outcome.get().is_none(),"框选结束不能开始编码、复制或释放冻结帧");
}

fn center(rect: PixelRect) -> (i32,i32) {(rect.left+rect.width()/2,rect.top+rect.height()/2)}
fn toolbar(session: &Session<'_>) -> ToolbarLayout {
    let selected=session.interaction.get().selection.unwrap();
    selection::toolbar_layout(selected.rect,session.bounds(selected.monitor),session.styles.borrow()[selected.monitor].metrics.selection)
}

#[test]
fn release_waits_for_confirmation_and_timestamp_starts_at_confirmation() {
    let cancel=AtomicBool::new(false); let session=session(&cancel);
    assert!(session.pointer_down(0,(100,100)));session.pointer_move(0,(500,400));
    session.confirm(); assert!(session.outcome.get().is_none(),"还在框选时 Enter 不提交");
    session.pointer_up(0,(500,400));
    assert!(session.outcome.get().is_none());
    assert_eq!(session.frames.len(),2,"调整期间保留冻结帧");
    let before=Instant::now();session.confirm();let after=Instant::now();
    match session.outcome.get().unwrap() {
        Outcome::Selected {monitor,rect,confirmed_at} => {
            assert_eq!(monitor,0);assert_eq!((rect.width(),rect.height()),(400,300));
            assert!(confirmed_at>=before && confirmed_at<=after);
        }
        other=>panic!("确认应生成已选结果：{other:?}"),
    }
}

#[test]
fn save_requires_down_and_up_on_same_button_and_monitor() {
    let cancel=AtomicBool::new(false); let session=session(&cancel);
    draw(&session,0,(200,200),(600,500));
    let buttons=toolbar(&session);let save=center(buttons.save);let cancel_button=center(buttons.cancel);
    session.pointer_up(0,save);assert!(session.outcome.get().is_none(),"单独松开不能保存");
    assert!(session.pointer_down(0,save));session.confirm();assert!(session.outcome.get().is_none(),"按钮尚未松开时 Enter 不提交");
    session.pointer_up(0,cancel_button);assert!(session.outcome.get().is_none(),"不同按钮不能算点击");
    assert!(session.pointer_down(0,save));session.pointer_up(1,save);assert!(session.outcome.get().is_none(),"不同屏不能算同按钮");
    assert!(session.pointer_down(0,save));session.pointer_move(0,(-50,-50));session.pointer_up(0,(-50,-50));
    assert!(session.outcome.get().is_none(),"按住后拖出松开不能保存");
    assert!(session.pointer_down(0,save));session.pointer_up(0,save);
    assert!(matches!(session.outcome.get(),Some(Outcome::Selected {monitor:0,..})));
}

#[test]
fn cancel_button_and_cancel_events_never_select_an_image() {
    for phase in 0..4 {
        let cancel=AtomicBool::new(false);let session=session(&cancel);
        if phase>0 {session.pointer_down(0,(100,100));session.pointer_move(0,(500,400));}
        if phase>1 {session.pointer_up(0,(500,400));}
        if phase==3 {session.pointer_down(0,(300,250));session.pointer_move(0,(340,270));}
        session.finish(Outcome::Cancelled);session.confirm();
        assert!(matches!(session.outcome.get(),Some(Outcome::Cancelled)),"Esc/右键取消不得被后续确认覆盖");
    }
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(600,500));
    let point=center(toolbar(&session).cancel);
    session.pointer_down(0,point);session.pointer_up(0,point);
    assert!(matches!(session.outcome.get(),Some(Outcome::Cancelled)));
}

#[test]
fn tiny_selection_disables_save_but_can_be_resized_then_confirmed() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(300,300),(400,400));
    // 拖右下角控制点把选区缩到 1 × 1：仍可调整，但不能保存。
    session.pointer_down(0,(400,400));session.pointer_move(0,(301,301));session.pointer_up(0,(301,301));
    assert_eq!(session.interaction.get().selection.unwrap().rect,PixelRect {left:300,top:300,right:301,bottom:301});
    session.confirm();assert!(session.outcome.get().is_none());
    let point=center(toolbar(&session).save);session.pointer_down(0,point);session.pointer_up(0,point);
    assert!(session.outcome.get().is_none());
    let state=session.interaction.get();
    let layout=overlay_layout(&session.frames[0],&session.styles.borrow()[0],state.visual()).unwrap();
    assert!(!layout.save_enabled);assert_eq!(layout.handles.len(),8);
    session.pointer_down(0,(301,301));session.pointer_move(0,(360,350));session.pointer_up(0,(360,350));
    assert!(session.interaction.get().selection.unwrap().rect.valid());session.confirm();
    assert!(matches!(session.outcome.get(),Some(Outcome::Selected {..})));
}

#[test]
fn eight_controls_flip_and_move_use_original_drag_anchor_and_locked_bounds() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);
    let original=PixelRect {left:200,top:200,right:600,bottom:500};
    let metrics=session.styles.borrow()[0].metrics.selection;
    for (grip,handle) in selection::handles(original,metrics) {
        session.set_interaction(Interaction {selection:Some(Selection {monitor:0,rect:original,adjustable:true}),..Default::default()});
        let start=center(handle);assert!(session.pointer_down(0,start));
        assert!(matches!(session.interaction.get().gesture,Some(Gesture::Transform {grip:actual,..}) if actual==grip));
        let end=(start.0+550,start.1+450);session.pointer_move(0,end);
        assert_eq!(session.interaction.get().selection.unwrap().rect,selection::apply_drag(original,grip,(550,450),session.bounds(0)));
        session.pointer_move(0,start);session.pointer_up(0,start);
        assert_eq!(session.interaction.get().selection.unwrap().rect,original,"越过对边后拖回原处，锚点不应跳变");
        assert!(session.outcome.get().is_none());
    }
    session.pointer_down(0,(400,350));session.pointer_move(0,(4000,-400));session.pointer_up(0,(4000,-400));
    let moved=session.interaction.get().selection.unwrap();
    assert_eq!((moved.rect.width(),moved.rect.height()),(400,300));assert_eq!((moved.rect.right,moved.rect.top),(1200,0));
}

#[test]
fn outside_reselection_switches_screen_and_uses_that_screens_dpi() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(600,500));
    session.pointer_down(0,(900,700));
    assert!(session.interaction.get().selection.unwrap().adjustable,"按下还没拖动时保留原选区");
    session.pointer_move(0,(1000,750));
    assert!(!session.interaction.get().selection.unwrap().adjustable,"拖动超过抖动阈值才开始画新框");session.pointer_up(0,(1000,750));
    assert_eq!(session.interaction.get().selection.unwrap().rect,PixelRect {left:900,top:700,right:1000,bottom:750});
    session.pointer_down(1,(50,50));session.pointer_move(1,(450,350));session.pointer_up(1,(450,350));
    let selected=session.interaction.get().selection.unwrap();assert_eq!(selected.monitor,1);
    assert_eq!((selected.rect.width(),selected.rect.height()),(400,300),"200% DPI 不倍乘选区物理像素");
    let layout=overlay_layout(&session.frames[1],&session.styles.borrow()[1],session.interaction.get().visual()).unwrap();
    assert!(layout.handles.iter().all(|handle|handle.width()==16));assert!(session.outcome.get().is_none());
}

#[test]
fn double_click_accepts_small_jitter_but_rejects_controls_and_real_drag() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(600,500));
    assert!(!session.double_click(0,(400,350)),"须有内部第一次 click，不能把刚框完的鼠标序列当双击");
    session.pointer_down(0,(400,350));session.pointer_up(0,(401,350));
    assert_eq!(session.interaction.get().selection.unwrap().rect.left,200,"点击抖动不移动选区");
    assert!(!session.double_click(0,(200,200)),"控制点不能透传双击保存");
    assert!(!session.double_click(0,center(toolbar(&session).save)),"按钮不能透传双击保存");
    session.pointer_down(0,(400,350));session.pointer_move(0,(450,350));session.pointer_move(0,(400,350));session.pointer_up(0,(400,350));
    assert!(!session.double_click(0,(400,350)),"真正拖动后回到原位仍不能算双击");
    session.pointer_down(0,(400,350));session.pointer_up(0,(400,350));assert!(session.double_click(0,(400,350)));
    assert!(matches!(session.outcome.get(),Some(Outcome::Selected {..})));
}

#[test]
fn signed_mouse_message_coordinates_and_flipped_resize_cursors_are_correct() {
    let lp=LPARAM(((-42i16 as u16 as u32)|((-300i16 as u16 as u32)<<16)) as isize);
    assert_eq!(message_point(lp),(-42,-300));
    let rect=PixelRect {left:100,top:100,right:500,bottom:400};
    assert_eq!(cursor_for_hit(Hit::Grip(Grip::NorthWest)),IDC_SIZENWSE);
    assert_eq!(cursor_for_hit(Hit::Grip(transformed_grip(Grip::NorthWest,rect,(100,100),(600,100)))),IDC_SIZENESW);
    assert_eq!(cursor_for_hit(Hit::Grip(transformed_grip(Grip::NorthWest,rect,(100,100),(600,500)))),IDC_SIZENWSE);
    assert_eq!(cursor_for_hit(Hit::Inside),IDC_SIZEALL);assert_eq!(cursor_for_hit(Hit::Save),IDC_HAND);
}

#[test]
fn stray_click_or_tiny_drag_outside_keeps_the_adjusted_selection() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(600,500));
    let kept=session.interaction.get().selection.unwrap();
    // 单击、抖动、拖出 1 像素宽的细条：都恢复原选区，不出现 0 尺寸工具栏。
    for (down,up) in [((900,700),(900,700)),((900,700),(903,702)),((900,700),(1100,701))] {
        session.pointer_down(0,down);session.pointer_move(0,up);session.pointer_up(0,up);
        assert_eq!(session.interaction.get().selection,Some(kept),"{down:?}→{up:?}");
        assert!(session.interaction.get().gesture.is_none());assert!(session.outcome.get().is_none());
    }
    // 在另一块屏误点也不丢。
    session.pointer_down(1,(50,50));session.pointer_up(1,(50,50));
    assert_eq!(session.interaction.get().selection,Some(kept));
}

#[test]
fn stray_click_before_any_selection_returns_to_the_hint() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);
    session.pointer_down(0,(300,300));session.pointer_up(0,(300,300));
    assert_eq!(session.interaction.get().selection,None);
    let layout=overlay_layout(&session.frames[0],&session.styles.borrow()[0],session.interaction.get().visual()).unwrap();
    assert!(layout.toolbar.is_none() && layout.handles.is_empty());
    session.pointer_down(0,(300,300));session.pointer_move(0,(301,340));session.pointer_up(0,(301,340));
    assert_eq!(session.interaction.get().selection,None,"不足 2 像素宽同样回到提示");
}

fn click(session: &Session<'_>, monitor: usize, point: (i32,i32)) { session.pointer_down(monitor,point); session.pointer_up(monitor,point); }
fn drag(session: &Session<'_>, monitor: usize, from: (i32,i32), to: (i32,i32)) {
    session.pointer_down(monitor,from);
    for step in 1..=4 { session.pointer_move(monitor,(from.0+(to.0-from.0)*step/4,from.1+(to.1-from.1)*step/4)); }
    session.pointer_up(monitor,to);
}
fn choose(session: &Session<'_>, tool: Tool) {
    let index = Tool::ALL.iter().position(|candidate| *candidate == tool).unwrap();
    click(session, 0, center(toolbar(session).tools[index]));
}
fn annotated_pixel(session: &Session<'_>, monitor: usize, point: (i32,i32)) -> [u8;3] {
    let annotated = session.annotated.borrow();
    let image = annotated[monitor].as_ref().expect("有标注时应有已标注帧");
    unsafe { let _ = GdiFlush(); }
    let bits = unsafe { std::slice::from_raw_parts(image.bits, byte_length(image.width,image.height).unwrap()) };
    let i = ((point.1*image.width+point.0)*4) as usize;
    [bits[i+2],bits[i+1],bits[i]]
}
fn is_red(rgb: [u8;3]) -> bool { rgb[0] > 200 && rgb[1] < 120 && rgb[2] < 120 }

#[test]
fn tool_buttons_toggle_and_undo_is_disabled_until_something_is_drawn() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(700,500));
    choose(&session,Tool::Box);
    assert_eq!(session.interaction.get().tool,Some(Tool::Box));
    choose(&session,Tool::Box);
    assert_eq!(session.interaction.get().tool,None,"再点一次取消选中");
    choose(&session,Tool::Arrow);choose(&session,Tool::Pen);
    assert_eq!(session.interaction.get().tool,Some(Tool::Pen),"同时只有一个工具");
    click(&session,0,center(toolbar(&session).undo));
    assert!(session.marks.borrow().is_empty());
    let layout=overlay_layout(&session.frames[0],&session.styles.borrow()[0],session.interaction.get().visual()).unwrap();
    assert!(!layout.undo_enabled && layout.tool==Some(Tool::Pen));
    assert!(session.outcome.get().is_none(),"工具按钮不保存也不取消");
}

#[test]
fn drawing_a_box_bakes_red_pixels_into_the_frame_and_undo_removes_them() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(700,500));
    choose(&session,Tool::Box);
    drag(&session,0,(300,260),(500,420));
    assert_eq!(session.marks.borrow().len(),1);
    assert!(is_red(annotated_pixel(&session,0,(300,340))),"框的左边是红色：{:?}",annotated_pixel(&session,0,(300,340)));
    assert!(!is_red(annotated_pixel(&session,0,(400,340))),"框内不填充");
    let layout=overlay_layout(&session.frames[0],&session.styles.borrow()[0],session.interaction.get().visual()).unwrap();
    assert!(layout.undo_enabled);
    // 选了工具时：选区内拖动不移动选区，选区外按下不重选，双击不保存。
    let before=session.interaction.get().selection.unwrap().rect;
    drag(&session,0,(250,230),(260,240));
    assert_eq!(session.interaction.get().selection.unwrap().rect,before);
    assert_eq!(session.marks.borrow().len(),2,"选区内拖动画了第二个框，而不是移动选区");
    click(&session,0,(900,700));
    assert_eq!(session.interaction.get().selection.unwrap().rect,before);
    assert!(!session.double_click(0,(400,350)));
    assert!(session.outcome.get().is_none());
    // 撤销按顺序撤掉（Ctrl+Z 走同一个函数）
    session.undo();
    assert_eq!(session.marks.borrow().len(),1);
    assert!(is_red(annotated_pixel(&session,0,(300,340))),"第一个框还在");
    session.undo();
    assert!(session.marks.borrow().is_empty() && session.annotated.borrow()[0].is_none());
}

#[test]
fn tiny_strokes_are_dropped_and_control_points_still_resize_with_a_tool() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(700,500));
    choose(&session,Tool::Arrow);
    click(&session,0,(400,300));
    assert!(session.marks.borrow().is_empty(),"误点不留痕");
    drag(&session,0,(700,500),(760,560));
    assert_eq!(session.interaction.get().selection.unwrap().rect,PixelRect {left:200,top:200,right:760,bottom:560},"右下角控制点仍可调整");
    drag(&session,0,(300,300),(600,450));
    assert_eq!(session.marks.borrow().len(),1);
    assert!(matches!(session.marks.borrow()[0].shape,Shape::Arrow((300,300),(600,450))));
}

#[test]
fn pen_strokes_record_points_and_reselecting_clears_marks() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(200,200),(700,500));
    choose(&session,Tool::Pen);
    drag(&session,0,(250,250),(450,330));
    let Shape::Pen(points)=session.marks.borrow()[0].shape.clone() else { panic!("应为画笔") };
    assert!(points.len()>=4 && points[0]==(250,250));
    // 标注点夹在选区内
    drag(&session,0,(650,450),(1100,790));
    let Shape::Pen(points)=session.marks.borrow()[1].shape.clone() else { panic!("应为画笔") };
    assert!(points.iter().all(|p|p.0<700 && p.1<500));
    // 取消工具后重新框选：标注清空
    choose(&session,Tool::Pen);
    drag(&session,0,(800,600),(1000,750));
    assert!(session.marks.borrow().is_empty() && session.annotated.borrow().iter().all(Option::is_none));
}

#[test]
fn mosaic_turns_the_area_into_uniform_blocks() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(100,100),(900,700));
    choose(&session,Tool::Mosaic);
    drag(&session,0,(200,200),(400,320));
    let block=session.anno_style(0).block;
    let cell=(200/block*block+block, 200/block*block+block);
    let a=annotated_pixel(&session,0,cell);
    let b=annotated_pixel(&session,0,(cell.0+block-1,cell.1+block-1));
    assert_eq!(a,b,"同一块内颜色一致");
    let outside=annotated_pixel(&session,0,(600,600));
    let image=&session.frames[0].image;
    let bits=unsafe{std::slice::from_raw_parts(image.bits,byte_length(image.width,image.height).unwrap())};
    let i=((600*image.width+600)*4) as usize;
    assert_eq!(outside,[bits[i+2],bits[i+1],bits[i]],"打码区域外不变");
}

#[test]
fn text_box_commits_on_enter_and_cancels_only_the_text_on_escape() {
    let cancel=AtomicBool::new(false);let session=session(&cancel);draw(&session,0,(100,100),(900,700));
    choose(&session,Tool::Text);
    click(&session,0,(300,300));
    let edit=session.edit.borrow().as_ref().map(|edit|edit.hwnd).expect("点击后出现输入框");
    unsafe { SetWindowTextW(edit,w!("注意这里")).unwrap(); }
    session.commit_text();
    assert!(session.edit.borrow().is_none());
    assert!(matches!(&session.marks.borrow()[0].shape,Shape::Text {text,at:(300,300),size} if text=="注意这里" && size.0>0 && size.1>0));
    let red=(300..420).any(|x|(300..340).any(|y|is_red(annotated_pixel(&session,0,(x,y)))));
    assert!(red,"文字以红色画进截图");
    // Esc：只取消这段文字，不取消截图
    click(&session,0,(300,500));
    let edit=session.edit.borrow().as_ref().map(|edit|edit.hwnd).unwrap();
    unsafe { SetWindowTextW(edit,w!("不要")).unwrap(); }
    session.cancel_text();
    assert_eq!(session.marks.borrow().len(),1);
    assert!(session.outcome.get().is_none());
    // 空文字不留痕；输入框开着时点「保存」先完成文字再保存
    click(&session,0,(500,500));
    session.commit_text();
    assert_eq!(session.marks.borrow().len(),1);
    click(&session,0,(500,550));
    let edit=session.edit.borrow().as_ref().map(|edit|edit.hwnd).unwrap();
    unsafe { SetWindowTextW(edit,w!("保存前")).unwrap(); }
    click(&session,0,center(toolbar(&session).save));
    assert_eq!(session.marks.borrow().len(),2);
    assert!(matches!(session.outcome.get(),Some(Outcome::Selected {..})));
}

/// 导出标注效果图：走真实的会话与绘制代码，背景为合成像素。
#[test]
#[ignore = "按需导出截图标注的效果图：设置 DAOZAI_ANNOTATE_PREVIEW 为输出目录"]
fn annotation_overlay_gallery() {
    let directory=std::path::PathBuf::from(std::env::var_os("DAOZAI_ANNOTATE_PREVIEW").expect("需指定输出目录"));
    std::fs::create_dir_all(&directory).unwrap();
    let cancel=AtomicBool::new(false);let session=session(&cancel);
    // 第二块屏是 200%：线宽、字号和工具栏按 DPI 放大。
    draw(&session,1,(120,90),(1080,620));
    let box_tool=Tool::ALL.iter().position(|tool|*tool==Tool::Box).unwrap();
    let selected=session.interaction.get().selection.unwrap();
    let toolbar=selection::toolbar_layout(selected.rect,session.bounds(1),session.styles.borrow()[1].metrics.selection);
    click(&session,1,center(toolbar.tools[box_tool]));
    drag(&session,1,(180,140),(520,330));
    let mut interaction=session.interaction.get();interaction.tool=Some(Tool::Arrow);session.set_interaction(interaction);
    drag(&session,1,(780,180),(560,280));
    let mut interaction=session.interaction.get();interaction.tool=Some(Tool::Pen);session.set_interaction(interaction);
    session.pointer_down(1,(200,420));
    for step in 0..=40 { let t=step as f32/40.0; session.pointer_move(1,((200.0+t*320.0) as i32,(420.0+(t*12.0).sin()*40.0) as i32)); }
    session.pointer_up(1,(520,420));
    let mut interaction=session.interaction.get();interaction.tool=Some(Tool::Mosaic);session.set_interaction(interaction);
    drag(&session,1,(640,400),(980,560));
    let mut interaction=session.interaction.get();interaction.tool=Some(Tool::Text);session.set_interaction(interaction);
    click(&session,1,(600,120));
    let edit=session.edit.borrow().as_ref().map(|edit|edit.hwnd).unwrap();
    unsafe { SetWindowTextW(edit,w!("这里填错了")).unwrap(); }
    session.commit_text();
    let mut interaction=session.interaction.get();interaction.tool=Some(Tool::Box);session.set_interaction(interaction);
    let frame=&session.frames[1];let bounds=session.bounds(1);
    let layout=overlay_layout(frame,&session.styles.borrow()[1],session.interaction.get().visual()).unwrap();
    let annotated=session.annotated.borrow();
    let painted=unsafe {render_dirty_with(frame,&session.dark_frames[1],annotated[1].as_ref(),&session.styles.borrow()[1],&layout,None,bounds).unwrap()};
    let rgba=painted.crop(bounds).unwrap();
    image::save_buffer_with_format(directory.join("200-annotated.png"),&rgba,bounds.width() as u32,bounds.height() as u32,image::ColorType::Rgba8,image::ImageFormat::Png).unwrap();
    let saved=annotated[1].as_ref().unwrap().crop(selected.rect).unwrap();
    image::save_buffer_with_format(directory.join("200-saved.png"),&saved,selected.rect.width() as u32,selected.rect.height() as u32,image::ColorType::Rgba8,image::ImageFormat::Png).unwrap();
}
