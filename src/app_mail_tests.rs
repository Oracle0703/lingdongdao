use super::*;

struct Fixture { app: App, previous: Option<std::ffi::OsString> }
impl Fixture {
    fn new() -> Self {
        let previous = std::env::var_os("DAOZAI_DATA_DIR");
        let path = std::env::temp_dir().join(format!("daozai-mail-regression-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::env::set_var("DAOZAI_DATA_DIR", path);
        let mut app = App::new(&egui::Context::default(), Options { fetch_lyrics: false, ..Default::default() });
        app.settings.mail_enabled = true;
        app.mail.accounts = vec![mail::Account { email: "test@example.com".into(), provider: mail::Provider::Qq, custom: None, enabled: true }];
        Self { app, previous }
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
