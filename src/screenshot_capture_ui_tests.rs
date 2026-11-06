use super::*;

fn escape() -> egui::Event {
    egui::Event::Key {key:egui::Key::Escape,physical_key:None,pressed:true,repeat:false,modifiers:Default::default()}
}

fn receive_code(h:&mut H,uid:u32) {
    h.app.mail.accounts=vec![crate::mail::Account {email:"capture@example.com".into(),provider:crate::mail::Provider::Qq,custom:None,enabled:true}];
    h.app.mail_event_at(crate::mail::Event::Mail {item:crate::mail::Item {
        account:"capture@example.com".into(),from:"截图测试".into(),subject:"登录验证码".into(),time:"09:41".into(),
        received_unix:Some(chrono::Utc::now().timestamp()),code:Some(format!("{uid:06}")),summary:"合成邮件".into(),
        links:vec![],meeting:None,uid,
    },new:true,updates:vec![]},h.t);
}

#[test]
fn screenshot_capture_suppresses_hover_and_restores_it_after_cleanup() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.show_page(0);h.hover(H::local(300.0,60.0),0.8);assert!(h.app.open);
    assert!(h.app.begin_screenshot());assert!(!h.app.open);
    assert!(!h.app.begin_screenshot(),"重复热键不能开始第二次截图");
    h.hover(Pos2::new(WIN_W/2.0,12.0),1.2);
    assert!(!h.app.open && h.app.progress()<0.01,"在岛位置停留不能展开");
    assert_eq!(h.app.page(),0);assert!(!h.hit.is_positive());assert!(h.file_drop_zone.is_none());
    h.app.end_screenshot();h.app.end_screenshot();
    assert!(!h.app.is_screenshot_capturing());
    h.hover(Pos2::new(WIN_W/2.0,12.0),0.8);assert!(h.app.open,"清理后恢复正常悬停");
}

#[test]
fn screenshot_capture_preserves_editor_drafts_and_independent_windows() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.show_page(4);h.hover(H::local(300.0,60.0),0.8);
    h.app.adding=true;h.app.buf="截图后继续编辑".into();h.app.settings_open=true;
    receive_code(&mut h,123456);h.app.mail.reading=h.app.mail.recent.first().cloned();
    assert!(h.app.begin_screenshot());h.app.foreground_changed();h.frame(vec![escape()]);
    h.hover(Pos2::new(WIN_W/2.0,12.0),1.0);
    assert!(h.app.adding && h.app.settings_open && h.app.mail.reading.is_some());
    assert_eq!(h.app.buf,"截图后继续编辑");assert_eq!(h.app.page(),4);
    h.app.end_screenshot();assert!(h.app.adding);assert_eq!(h.app.buf,"截图后继续编辑");
    h.app.settings_open=false;h.hover(Pos2::new(WIN_W/2.0,12.0),0.8);
    assert!(h.app.adding);assert_eq!(h.app.buf,"截图后继续编辑");
    // 构造阅读窗口时投递的新邮件会在截图结束后显示原有的通知横幅。
    // 横幅期间原有规则不允许悬停展开；先确认草稿保留，再等横幅自然结束。
    let notice=h.app.notice.as_ref().expect("截图期间的新邮件通知应延后显示");
    assert_eq!(notice.title,"新邮件");
    h.t=notice.at+notice.duration+0.1;h.frame(vec![]);
    h.hover(Pos2::new(WIN_W/2.0,12.0),0.8);
    assert!(h.app.open,"通知结束后应恢复正常悬停展开");
    assert!(h.app.adding);assert_eq!(h.app.buf,"截图后继续编辑");

    // 连空白待办也不能被遮罩取得前台的事件误取消。
    h.app.buf.clear();assert!(h.app.begin_screenshot());h.app.foreground_changed();
    h.frame(vec![escape()]);h.app.end_screenshot();assert!(h.app.adding);
}

#[test]
fn screenshot_capture_preserves_calendar_draft_and_rejects_task_dialog_or_drag() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.open_calendar_editor(None);
    h.app.calendar.draft.as_mut().unwrap().title="未保存日程".into();
    assert!(h.app.begin_screenshot());h.frame(vec![escape()]);h.app.end_screenshot();
    assert_eq!(h.app.calendar.draft.as_ref().unwrap().title,"未保存日程");
    h.app.cancel_edit();
    h.app.todos.items.push(crate::model::TodoItem{text:"编辑任务".into(),done_on:None,due:None,reminded:false});
    h.app.open_task_dialog(0);assert!(!h.app.begin_screenshot());assert!(h.app.task_dialog.is_some());
    h.app.cancel_edit();h.app.drag_hot=true;
    assert!(!h.app.begin_screenshot());assert!(!h.app.is_screenshot_capturing());
}

#[test]
fn screenshot_capture_defers_latest_code_and_reminders_until_cleanup() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.frame(vec![]);h.cmds.clear();assert!(h.app.begin_screenshot());
    receive_code(&mut h,123456);receive_code(&mut h,654321);
    h.app.alert("待办到时间了","合成任务",4,true);
    h.hover(Pos2::new(WIN_W/2.0,12.0),1.0);
    assert!(!h.app.open && h.app.mail.preview.is_none() && h.app.notice.is_none());
    assert_eq!(h.app.mail.recent.len(),2,"后台收信继续更新");
    assert!(!h.cmds.iter().any(|cmd|matches!(cmd,crate::app::Cmd::Notify(..)|crate::app::Cmd::Focus)));
    h.app.end_screenshot();h.frame(vec![]);
    assert!(h.app.open);assert_eq!(h.app.mail.preview.as_ref().unwrap().item.uid,654321);
    assert!(h.app.notice.is_some());
    assert_eq!(h.cmds.iter().filter(|cmd|matches!(cmd,crate::app::Cmd::Notify(title,_) if title=="待办到时间了")).count(),1);
}

#[test]
fn screenshot_capture_does_not_revive_reset_or_removed_mail() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.frame(vec![]);assert!(h.app.begin_screenshot());receive_code(&mut h,123456);
    h.app.mail_event(crate::mail::Event::MailboxReset{account:"capture@example.com".into()});
    h.app.end_screenshot();assert!(h.app.mail.preview.is_none());
    assert!(h.app.begin_screenshot());receive_code(&mut h,654321);h.app.mail.accounts.clear();
    h.app.end_screenshot();assert!(h.app.mail.preview.is_none());
}

#[test]
fn screenshot_capture_defers_independent_window_focus_without_closing_windows() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(|ctx,mut viewport|{
        let id=viewport.ids.this;
        let mut raw=egui::RawInput {viewport_id:id,screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(760.0,640.0))),..Default::default()};
        raw.viewports.insert(id,egui::ViewportInfo::default());
        let _=ctx.run(raw,|ctx|(viewport.viewport_ui_cb)(ctx));
    });
    let frame=|h:&mut H|h.ctx.run(Default::default(),|ctx|{
        h.app.settings_ui(ctx,false);h.app.mail_reader_ui(ctx);
    });
    h.app.open_settings();receive_code(&mut h,123456);
    h.app.mail.reading=h.app.mail.recent.first().cloned();h.app.mail.reader_focus=true;
    let ids=[egui::ViewportId::from_hash_of("daozai-settings"),egui::ViewportId::from_hash_of("daozai-mail-reader")];
    let initial=frame(&mut h);
    for id in ids {assert!(initial.viewport_output[&id].commands.iter().any(|cmd|matches!(cmd,egui::ViewportCommand::Focus)));}
    let mut builders=ids.map(|id|initial.viewport_output[&id].builder.clone());
    // 对照旧实现，证明测试能捕获 active 随截图状态变化导致的窗口重建。
    let mut old_mail_builder=builders[1].clone().with_active(true);
    assert!(old_mail_builder.patch(builders[1].clone().with_active(false)).1);
    assert!(old_mail_builder.patch(builders[1].clone().with_active(true)).1);
    let preserves_window=|existing:&mut egui::ViewportBuilder,viewport:&egui::ViewportOutput|{
        let (delta,recreate_window)=existing.patch(viewport.builder.clone());
        assert!(!recreate_window,"截图开始和结束都必须保留原生窗口");
        assert!(!delta.iter().any(|cmd|matches!(cmd,egui::ViewportCommand::Focus|egui::ViewportCommand::Visible(false))),"builder 更新不能隐藏窗口或隐式抢焦点");
        assert!(!delta.iter().chain(viewport.commands.iter()).any(|cmd|matches!(cmd,
            egui::ViewportCommand::OuterPosition(..)|egui::ViewportCommand::InnerSize(..)
            |egui::ViewportCommand::MinInnerSize(..)|egui::ViewportCommand::MaxInnerSize(..)
            |egui::ViewportCommand::ResizeIncrements(..))),"截图不得重置窗口位置或尺寸");
    };
    h.app.settings_focus_requested=true;h.app.mail.reader_focus=true;
    assert!(h.app.begin_screenshot());
    for _ in 0..2 {
        let output=frame(&mut h);
        for (id,builder) in ids.into_iter().zip(&mut builders) {
            let viewport=&output.viewport_output[&id];
            preserves_window(builder,viewport);
            assert!(!viewport.commands.iter().any(|cmd|matches!(cmd,egui::ViewportCommand::Focus|egui::ViewportCommand::Visible(false))));
        }
        assert!(h.app.settings_open && h.app.mail.reading.is_some());
        assert!(h.app.settings_focus_requested && h.app.mail.reader_focus,"截图期间不能丢弃焦点请求");
    }
    h.app.end_screenshot();
    for expected in [true,false] {
        let output=frame(&mut h);
        for (id,builder) in ids.into_iter().zip(&mut builders) {
            let viewport=&output.viewport_output[&id];
            preserves_window(builder,viewport);
            assert_eq!(viewport.commands.iter().any(|cmd|matches!(cmd,egui::ViewportCommand::Focus)),expected);
        }
    }
}

#[test]
fn screenshot_capture_defers_first_mail_window_and_resets_after_close() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.ctx.set_embed_viewports(false);
    let close_next=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let close=close_next.clone();
    egui::Context::set_immediate_viewport_renderer(move|ctx,mut viewport|{
        let id=viewport.ids.this;
        let mut raw=egui::RawInput {viewport_id:id,screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(700.0,620.0))),..Default::default()};
        let mut info=egui::ViewportInfo::default();
        if close.swap(false,std::sync::atomic::Ordering::SeqCst) {info.events.push(egui::ViewportEvent::Close);}
        raw.viewports.insert(id,info);
        let _=ctx.run(raw,|ctx|(viewport.viewport_ui_cb)(ctx));
    });
    let frame=|h:&mut H|h.ctx.run(Default::default(),|ctx|h.app.mail_reader_ui(ctx));
    let id=egui::ViewportId::from_hash_of("daozai-mail-reader");
    receive_code(&mut h,123456);
    for close_by_window in [true,false] {
        assert!(!h.app.mail.reader_presented);
        h.app.mail.reading=h.app.mail.recent.first().cloned();h.app.mail.reader_focus=true;
        assert!(h.app.begin_screenshot());
        for _ in 0..2 {
            let output=frame(&mut h);
            assert!(!output.viewport_output.contains_key(&id),"截图中不能首次创建邮件窗口");
            assert!(h.app.mail.reading.is_some() && h.app.mail.reader_focus);
            assert!(!h.app.mail.reader_presented);
        }
        h.app.end_screenshot();
        for expected_focus in [true,false] {
            let output=frame(&mut h);
            let viewport=&output.viewport_output[&id];
            assert_eq!(viewport.commands.iter().any(|cmd|matches!(cmd,egui::ViewportCommand::Focus)),expected_focus);
            assert!(h.app.mail.reader_presented);
        }
        if close_by_window {close_next.store(true,std::sync::atomic::Ordering::SeqCst);}
        else {h.app.mail.reading=None;}
        frame(&mut h);
        assert!(h.app.mail.reading.is_none() && !h.app.mail.reader_presented,"窗口关闭或内容移除都应清理已呈现标记");
        frame(&mut h);
    }
}
