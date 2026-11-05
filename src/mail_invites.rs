//! 邮件邀请修订：正文继续保留，只有最新的邀请版本参与日历和提醒。
use super::{Item, Meeting};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarUpdate {
    pub uid: String,
    pub recurrence_id: Option<i64>,
    pub sequence: u32,
    pub stamp: i64,
    pub organizer: String,
    pub meeting: Option<Meeting>,
}

pub(crate) fn parse(calendar: &str) -> Vec<CalendarUpdate> {
    super::calendar_fields(calendar).into_iter().filter_map(|(method, fields)| {
        if !matches!(method.as_str(), "" | "REQUEST" | "PUBLISH" | "CANCEL") { return None; }
        let get = |name: &str| fields.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        let uid = get("UID").filter(|value| !value.is_empty())?.to_owned();
        let recurrence_id = if get("RECURRENCE-ID").is_some() {
            Some(super::event_time(&fields, "RECURRENCE-ID")?.timestamp())
        } else { None };
        let cancelled = method == "CANCEL" || get("STATUS").is_some_and(|value| value.eq_ignore_ascii_case("CANCELLED"));
        let meeting = if cancelled { None } else { Some(super::event_of(&fields)?) };
        Some(CalendarUpdate {
            uid, recurrence_id, meeting,
            sequence: get("SEQUENCE").unwrap_or("0").parse().ok()?,
            stamp: super::event_time(&fields, "DTSTAMP").map(|at| at.timestamp()).unwrap_or(i64::MIN),
            organizer: get("ORGANIZER").unwrap_or("").to_ascii_lowercase(),
        })
    }).collect()
}

type Key = (String, String, Option<i64>);
#[derive(Default)]
pub(crate) struct Invitations {
    messages: HashMap<(String, u32), Vec<CalendarUpdate>>,
    latest: HashMap<Key, (u32, CalendarUpdate)>,
}

impl Invitations {
    pub fn observe(&mut self, account: &str, mail_uid: u32, updates: Vec<CalendarUpdate>) {
        let account = account.to_ascii_lowercase();
        for update in &updates {
            let key = (account.clone(), update.uid.clone(), update.recurrence_id);
            let rank = |uid, update: &CalendarUpdate| (update.sequence, update.stamp, uid);
            if self.latest.get(&key).is_none_or(|(old_uid, old)| {
                old.organizer == update.organizer && rank(mail_uid, update) >= rank(*old_uid, old)
            }) {
                self.latest.insert(key, (mail_uid, update.clone()));
            }
        }
        self.messages.insert((account, mail_uid), updates);
    }

    pub fn reconcile(&self, item: &mut Item) {
        let account = item.account.to_ascii_lowercase();
        let Some(updates) = self.messages.get(&(account.clone(), item.uid)) else { return; };
        item.meeting = updates.iter().find_map(|update| {
            let meeting = update.meeting.as_ref()?;
            let key = (account.clone(), update.uid.clone(), update.recurrence_id);
            let (uid, latest) = self.latest.get(&key)?;
            if *uid != item.uid || latest != update { return None; }
            if update.recurrence_id.is_some() {
                if let Some((_, master)) = self.latest.get(&(account.clone(), update.uid.clone(), None)) {
                    // 整场取消优先于更旧的单次邀请；新版本的单次修改可重新安排。
                    if master.organizer == update.organizer && master.meeting.is_none()
                        && (master.sequence, master.stamp) >= (update.sequence, update.stamp) { return None; }
                }
            } else if self.latest.get(&(account.clone(), update.uid.clone(), Some(meeting.start)))
                .is_some_and(|(_, instance)| instance.organizer == update.organizer
                    && (instance.sequence, instance.stamp) >= (update.sequence, update.stamp)) {
                return None;
            }
            Some(meeting.clone())
        });
    }

    pub fn reset(&mut self, account: &str) {
        let account = account.to_ascii_lowercase();
        self.messages.retain(|(owner, _), _| owner != &account);
        self.latest.retain(|(owner, _, _), _| owner != &account);
    }

    pub fn retain(&mut self, items: &[Item]) {
        self.messages.retain(|(account, uid), _| items.iter().any(|item| item.uid == *uid && item.account.eq_ignore_ascii_case(account)));
        // 保留仍有旧邀请时的取消墓碑，避免取消邮件先退出列表后旧日程复活。
        self.latest.retain(|(account, uid, _), _| self.messages.iter().any(|((owner, _), updates)|
            owner == account && updates.iter().any(|update| &update.uid == uid)));
    }
}
