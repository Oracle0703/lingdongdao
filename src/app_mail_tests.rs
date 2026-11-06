use super::*;

struct Fixture { app: App, previous: Option<std::ffi::OsString>, _env: std::sync::MutexGuard<'static, ()> }
impl Fixture {
    fn new() -> Self {
        let env = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        let path = std::env::temp_dir().join(format!("daozai-mail-regression-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::env::set_var("DAOZAI_DATA_DIR", path);
        let mut app = App::new(&egui::Context::default(), Options { fetch_lyrics: false, ..Default::default() });
        app.settings.mail_enabled = true;
        app.mail.accounts = vec![mail::Account { email: "test@example.com".into(), provider: mail::Provider::Qq, custom: None, enabled: true }];
        Self { app, previous, _env: env }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {std::env::set_var("DAOZAI_DATA_DIR", value);}
        else {std::env::remove_var("DAOZAI_DATA_DIR");}
    }
}

fn invitation(method: &str, sequence: u32, start: &str, extra: &str) -> String {
    format!("BEGIN:VCALENDAR\nMETHOD:{method}\nBEGIN:VEVENT\nUID:meeting-1\nORGANIZER:mailto:owner@example.com\nSEQUENCE:{sequence}\nDTSTAMP:20300101T000000Z\nSUMMARY:评审\n{start}\n{extra}\nEND:VEVENT\nEND:VCALENDAR")
}
fn receive(app: &mut App, uid: u32, calendar: &str) {
    let account = "test@example.com".to_owned();
    let updates = mail::invites::parse(calendar);
    app.mail_event(mail::Event::Mail { item: mail::Item {
        account, from: "owner@example.com".into(), subject: "邀请".into(), time: String::new(), code: None,
        received_unix: None, summary: "保留邮件正文".into(), links: vec![], meeting: mail::meeting_of(calendar), uid,
    }, new: false, updates });
}

#[test]
fn new_code_after_a_long_frame_pause_gets_its_full_local_display_window() {
    let mut fixture=Fixture::new();let app=&mut fixture.app;
    app.settings.mail_code_hide_seconds=60;
    app.t=1.0;
    let item=mail::Item {
        account:"test@example.com".into(),from:"测试".into(),subject:"验证码".into(),time:String::new(),
        code:Some("123456".into()),received_unix:Some(chrono::Utc::now().timestamp()-120),
        summary:"验证码 123456".into(),links:vec![],meeting:None,uid:1,
    };
    app.mail_event_at(mail::Event::Mail{item:item.clone(),new:true,updates:Vec::new()},300.0);
    assert!(!app.mail_code_hidden(&item),"恢复后不能用暂停前的旧时钟开始计时");
    app.t=359.9;assert!(!app.mail_code_hidden(&item));
    app.t=360.0;assert!(app.mail_code_hidden(&item));
    app.mail_event_at(mail::Event::Mail{item:item.clone(),new:true,updates:Vec::new()},370.0);
    assert!(app.mail_code_hidden(&item),"重复事件不能延长显示窗口");
}

#[test]
fn resumed_mail_backlog_keeps_old_codes_masked_until_manually_revealed() {
    let mut fixture=Fixture::new();let app=&mut fixture.app;
    app.settings.mail_code_hide_seconds=60;app.t=1.0;
    let item=mail::Item {
        account:"test@example.com".into(),from:"测试".into(),subject:"验证码 123456".into(),time:String::new(),
        code:Some("123456".into()),received_unix:Some(chrono::Utc::now().timestamp()-7200),
        summary:"验证码 123456".into(),links:vec![],meeting:None,uid:1,
    };
    app.mail_event_at(mail::Event::Mail{item:item.clone(),new:true,updates:Vec::new()},7200.0);
    let visible=app.display_mail(&item);
    assert!(visible.code.is_none()&&!visible.subject.contains("123456")&&!visible.summary.contains("123456"));
    app.reveal_mail_code(&item);assert!(app.display_mail(&item).code.is_some());
    app.t+=15.0;assert!(app.display_mail(&item).code.is_none());
}

fn preview_code(uid:u32)->mail::Item {
    mail::Item{account:"test@example.com".into(),from:"测试服务".into(),subject:"登录验证".into(),time:"14:00".into(),
        code:Some("482913".into()),received_unix:Some(chrono::Utc::now().timestamp()),
        summary:"验证码：482913，请勿泄露给他人。".into(),links:vec![],meeting:None,uid}
}
fn preview_receive(app:&mut App,item:mail::Item,new:bool,at:f64) {
    app.mail_event_at(mail::Event::Mail{item,new,updates:Vec::new()},at);
}

#[test]
fn mail_preview_is_temporary_and_duplicate_delivery_does_not_extend_it() {
    let mut fixture=Fixture::new();let app=&mut fixture.app;
    app.settings.notifications=false;app.page=SALARY_PAGE;
    let item=preview_code(1);preview_receive(app,item.clone(),true,100.0);
    assert!(app.open&&app.mail.preview.is_some(),"验证码预览不依赖普通通知开关");
    assert_eq!(app.page,SALARY_PAGE,"自动预览不能改掉用户上次看的页面");
    preview_receive(app,item.clone(),true,110.0);
    assert_eq!(app.mail.preview.as_ref().unwrap().until,130.0);
    app.t=129.9;app.tick_mail_preview();assert!(app.open);
    app.t=130.0;app.tick_mail_preview();
    assert!(!app.open&&app.mail.preview.is_none()&&app.hover_lock);
    assert_eq!(app.page,SALARY_PAGE);
    assert!(!app.mail_code_hidden(&item),"30 秒收起预览不应改变原有验证码隐藏时长");
}

#[test]
fn mail_preview_ignores_history_stale_codes_disabled_features_and_active_edits() {
    for scenario in 0..10 {
        let mut fixture=Fixture::new();let app=&mut fixture.app;
        let mut item=preview_code(1);let mut new=true;
        match scenario {
            0=>new=false,
            1=>item.code=None,
            2=>item.received_unix=Some(chrono::Utc::now().timestamp()-7200),
            3=>app.settings.mail_code=false,
            4=>app.settings.visible_pages[5]=false,
            5=>app.mail.accounts.clear(),
            6=>{app.adding=true;app.buf="未保存草稿".into();},
            7=>app.settings_open=true,
            8=>app.drag_hot=true,
            _=>{item.summary="验证码 482913 有效期为30秒".into();item.received_unix=Some(chrono::Utc::now().timestamp()-40);},
        }
        preview_receive(app,item,new,10.0);
        assert!(app.mail.preview.is_none(),"不应弹出的场景 {scenario}");
        if scenario==6 {assert_eq!(app.buf,"未保存草稿");assert!(app.adding);}
    }
}

#[test]
fn mail_preview_tracks_the_latest_code_across_accounts_and_ignores_ordinary_mail() {
    let mut fixture=Fixture::new();let app=&mut fixture.app;
    app.mail.accounts.push(mail::Account{email:"other@example.com".into(),provider:mail::Provider::Qq,custom:None,enabled:true});
    let first=preview_code(1);preview_receive(app,first.clone(),true,1.0);
    let mut second=preview_code(1);second.account="other@example.com".into();second.code=Some("975310".into());
    preview_receive(app,second.clone(),true,10.0);
    assert_eq!(app.mail.preview.as_ref().unwrap().until,40.0);
    preview_receive(app,first,true,11.0);
    let mut ordinary=preview_code(2);ordinary.code=None;preview_receive(app,ordinary,true,12.0);
    let shown=&app.mail.preview.as_ref().unwrap().item;
    assert_eq!(shown.account,second.account);assert_eq!(shown.code,second.code);
    app.mail_event(mail::Event::MailboxReset{account:second.account});
    assert!(app.mail.preview.is_none()&&!app.open);
}

#[test]
fn mail_preview_respects_earlier_privacy_deadlines_and_disabling_accounts() {
    for scenario in 0..4 {
        let mut fixture=Fixture::new();let app=&mut fixture.app;
        let mut item=preview_code(1);
        if scenario==0 {app.settings.mail_code_hide_seconds=15;}
        if scenario==1 {item.received_unix=Some(chrono::Utc::now().timestamp()-298);}
        if scenario==2 {item.summary="验证码 482913 有效期为20秒".into();}
        preview_receive(app,item,true,10.0);assert!(app.mail.preview.is_some());
        app.t=match scenario{0=>25.0,1=>13.0,2=>30.0,_=>11.0};
        if scenario==3 {app.mail.accounts[0].enabled=false;}
        app.tick_mail_preview();assert!(app.mail.preview.is_none()&&!app.open);
    }
}

#[test]
fn cancellation_retracts_loaded_invitation_and_pending_visible_reminders() {
    let mut fixture = Fixture::new(); let app = &mut fixture.app;
    let request = invitation("REQUEST", 1, "DTSTART:20300102T100000Z", "");
    receive(app, 1, &request);
    let meeting = app.mail.recent[0].meeting.clone().unwrap();
    let day = chrono::DateTime::from_timestamp(meeting.start, 0).unwrap().with_timezone(&chrono::Local).date_naive();
    assert_eq!(app.calendar_entries(day).len(), 1);
    let alert = Alert { title: "会议提醒".into(), source: crate::extras::AlertSource::Internal, body: String::new(), page: CALENDAR_PAGE, at: 0.0, duration: 12.0, bluetooth: None, meeting: Some((meeting, "邮件邀请".into())) };
    app.alerts.push_back(alert.clone()); app.notice = Some(alert.clone()); app.hud_notice = Some(alert);
    // RFC 5546 的取消可以不带 DTSTART，仍须按 UID 撤销原邀请。
    receive(app, 2, &invitation("CANCEL", 2, "", ""));
    assert!(app.calendar_entries(day).is_empty());
    assert!(app.mail.recent.iter().all(|item| item.meeting.is_none()));
    assert_eq!(app.mail.recent.len(), 2);
    assert!(app.mail.recent.iter().all(|item| item.summary.as_ref() == "保留邮件正文"));
    assert!(app.alerts.is_empty() && app.notice.is_none() && app.hud_notice.is_none());
    // 延迟到达的旧版本不能恢复已取消的会议。
    receive(app, 3, &request);
    assert!(app.calendar_entries(day).is_empty());
}

#[test]
fn rescheduling_replaces_old_time_and_ignores_stale_or_foreign_revisions() {
    let mut fixture = Fixture::new(); let app = &mut fixture.app;
    let old = invitation("REQUEST", 1, "DTSTART:20300102T100000Z", "");
    receive(app, 1, &old);
    receive(app, 2, &invitation("REQUEST", 2, "DTSTART:20300102T120000Z", ""));
    receive(app, 3, &old);
    receive(app, 4, &invitation("CANCEL", 3, "", "").replace("owner@example.com", "other@example.com"));
    let meetings = app.mail.recent.iter().filter_map(|item| item.meeting.as_ref()).collect::<Vec<_>>();
    assert_eq!(meetings.len(), 1);
    assert_eq!(meetings[0].start, chrono::DateTime::parse_from_rfc3339("2030-01-02T12:00:00Z").unwrap().timestamp());
    receive(app, 5, &invitation("REPLY", 3, "DTSTART:20300102T120000Z", "STATUS:CANCELLED"));
    assert_eq!(app.mail.recent.iter().filter(|item| item.meeting.is_some()).count(), 1);
}

#[test]
fn single_instance_cancellation_and_reschedule_use_recurrence_identity() {
    let mut fixture = Fixture::new(); let app = &mut fixture.app;
    receive(app, 1, &invitation("REQUEST", 1, "DTSTART:20300102T100000Z", "RECURRENCE-ID:20300102T100000Z"));
    receive(app, 2, &invitation("REQUEST", 1, "DTSTART:20300103T100000Z", "RECURRENCE-ID:20300103T100000Z"));
    receive(app, 3, &invitation("CANCEL", 2, "", "RECURRENCE-ID:20300102T100000Z"));
    assert_eq!(app.mail.recent.iter().filter(|item| item.meeting.is_some()).count(), 1);
    receive(app, 4, &invitation("REQUEST", 3, "DTSTART:20300102T140000Z", "RECURRENCE-ID:20300102T100000Z"));
    assert_eq!(app.mail.recent.iter().filter(|item| item.meeting.is_some()).count(), 2);
}

#[test]
fn mailbox_reset_clears_old_messages_and_invitation_versions_before_uid_reuse() {
    let mut fixture = Fixture::new(); let app = &mut fixture.app;
    receive(app, 100, &invitation("REQUEST", 9, "DTSTART:20300102T100000Z", ""));
    app.mail.open_uid = Some(("test@example.com".into(), 100));
    app.mail.reading = app.mail.recent.first().cloned();
    app.mail_event(mail::Event::MailboxReset { account: "test@example.com".into() });
    assert!(app.mail.recent.is_empty() && app.mail.open_uid.is_none() && app.mail.reading.is_none());
    receive(app, 1, &invitation("REQUEST", 1, "DTSTART:20300102T110000Z", ""));
    assert_eq!(app.mail.recent.len(), 1);
    assert!(app.mail.recent[0].meeting.is_some());
}

#[test]
fn other_accounts_mail_between_invitation_and_cancellation_keeps_revision_state() {
    let mut fixture = Fixture::new(); let app = &mut fixture.app;
    app.mail.accounts.push(mail::Account { email: "other@example.com".into(), provider: mail::Provider::Qq, custom: None, enabled: true });
    let request = invitation("REQUEST", 1, "DTSTART:20300102T100000Z", "");
    receive(app, 5, &request);
    // 另一个账号的邮件夹在邀请与取消之间到达，不能清掉本账号的邀请版本。
    app.mail_event(mail::Event::Mail { item: mail::Item {
        account: "other@example.com".into(), from: "someone@example.com".into(), subject: "普通邮件".into(), time: String::new(), code: None,
        received_unix: None, summary: "正文".into(), links: vec![], meeting: None, uid: 7,
    }, new: false, updates: Vec::new() });
    receive(app, 9, &invitation("CANCEL", 2, "", ""));
    assert!(app.mail.recent.iter().all(|item| item.meeting.is_none()), "取消邮件必须撤销同账号的原邀请");
}

fn assert_open_reader_retains_digest_after_mail_list_eviction(state: crate::mail_digest::Digest) {
    for reset_mailbox in [false, true] {
        let mut fixture = Fixture::new();
        let app = &mut fixture.app;
        let ctx = egui::Context::default();
        app.mail.digests.set_enabled(true);
        let old = mail::Item {
            account: "test@example.com".into(),
            from: "sender@example.com".into(),
            subject: "阅读中的旧邮件".into(),
            time: String::new(),
            code: None,
            received_unix: None,
            summary: "旧邮件正文应在阅读窗口继续保留。".into(),
            links: vec![],
            meeting: None,
            uid: 1,
        };
        app.mail_event(mail::Event::Mail {
            item: old.clone(),
            new: true,
            updates: vec![],
        });
        app.mail.reading = Some(old.clone());
        app.mail.digests.insert_for_test(&old, state.clone());

        for uid in 2..=76 {
            let mut incoming = old.clone();
            incoming.uid = uid;
            incoming.subject = format!("新增邮件 {uid}");
            app.mail_event(mail::Event::Mail {
                item: incoming,
                new: true,
                updates: vec![],
            });
            assert_eq!(app.mail.reading.as_ref(), Some(&old));
            assert_eq!(
                app.mail.digests.get(&old),
                Some(&state),
                "第 {uid} 封邮件到达后，仍打开的阅读窗口不能失去 {state:?} 摘要"
            );
            // 模拟阅读窗口后续渲染：Ready 命中缓存，Pending 不重复排队，Failed 不隐式重试。
            app.mail.digests.request(&old, &ctx);
            assert_eq!(app.mail.digests.get(&old), Some(&state));
            assert_eq!(app.mail.digests.request_count_for_test(), 0);
            assert!(!app.mail.digests.worker_started_for_test());
        }
        assert_eq!(app.mail.recent.len(), 60);
        assert!(app.mail.recent.iter().all(|item| item.uid != old.uid));

        if reset_mailbox {
            // UIDVALIDITY 重置必须连同独立阅读窗口及其列表外摘要一起失效。
            app.mail_event(mail::Event::MailboxReset { account: old.account.clone() });
            assert!(app.mail.recent.is_empty());
            assert!(app.mail.reading.is_none());
        } else {
            app.mail.reading = None;
            let mut incoming = old.clone();
            incoming.uid = 77;
            incoming.subject = "关闭阅读窗口后的新邮件".into();
            app.mail_event(mail::Event::Mail {
                item: incoming,
                new: true,
                updates: vec![],
            });
            assert_eq!(app.mail.recent.len(), 60);
        }
        assert!(
            app.mail.digests.get(&old).is_none(),
            "关闭阅读窗口后收到新邮件或邮箱重置后，旧摘要必须被清理：{state:?}"
        );
        assert_eq!(app.mail.digests.request_count_for_test(), 0);
        assert!(!app.mail.digests.worker_started_for_test());
    }
}

#[test]
fn open_reader_ready_digest_survives_mail_list_eviction() {
    assert_open_reader_retains_digest_after_mail_list_eviction(
        crate::mail_digest::Digest::Ready("请在明天下午参加评审。".into()),
    );
}

#[test]
fn open_reader_pending_digest_survives_mail_list_eviction() {
    assert_open_reader_retains_digest_after_mail_list_eviction(crate::mail_digest::Digest::Pending);
}

#[test]
fn open_reader_failed_digest_survives_mail_list_eviction() {
    assert_open_reader_retains_digest_after_mail_list_eviction(
        crate::mail_digest::Digest::Failed("模型服务暂时不可用，请稍后重试".into()),
    );
}
