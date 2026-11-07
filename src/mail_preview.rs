//! 新验证码的临时预览，不改变用户原先停留的页面。
use super::*;

pub(super) const HEIGHT:f32=180.0;
const DURATION:f64=30.0;

pub(crate) struct Preview {
    pub item:mail::Item,
    pub until:f64,
}

impl App {
    pub(super) fn start_mail_preview(&mut self,item:mail::Item) {
        if item.code.is_none() || !self.settings.mail_enabled || !self.settings.mail_code
            || !self.settings.visible_pages[5] || self.settings_open || self.is_editing()
            || self.drag_hot || self.browser_minimal_active || self.mail_code_hidden(&item) {return;}
        self.mail.preview=Some(Preview{item,until:self.t+DURATION});
        self.open=true;
        self.hover_lock=false;
        self.hover_since=None;
        self.leave_since=None;
        self.forced_open_until=0.0;
        self.w.tune(OPEN_W.0,OPEN_W.1);self.h.tune(OPEN_H.0,OPEN_H.1);
        // 截图预览开着时由它决定高度，验证码预览排在其后显示。
        self.w.target=PW;self.h.target=self.expanded_height();
    }

    pub(super) fn tick_mail_preview(&mut self) {
        let expired=self.mail.preview.as_ref().is_some_and(|p|self.t>=p.until
            || self.mail_code_hidden(&p.item) || !self.settings.mail_enabled || !self.settings.mail_code
            || !self.settings.visible_pages[5] || self.browser_minimal_active
            || !self.mail.accounts.iter().any(|a|a.enabled&&a.email.eq_ignore_ascii_case(&p.item.account)));
        if expired {self.end_mail_preview(true);}
    }

    pub(super) fn end_mail_preview(&mut self,collapse:bool) {
        if self.mail.preview.take().is_none() {return;}
        self.forced_open_until=0.0;
        if collapse {
            self.open=false;
            self.hover_lock=true;
            self.hover_since=None;
            self.leave_since=None;
            self.w.tune(CLOSE.0,CLOSE.1);self.h.tune(CLOSE.0,CLOSE.1);
            let (width,height)=self.base_size();self.w.target=width;self.h.target=height;
        }
    }

    pub(super) fn draw_mail_preview(&mut self,c:&Card,cmds:&mut Vec<Cmd>)->bool {
        let Some(preview)=&self.mail.preview else{return false;};
        let item=self.display_mail(&preview.item);
        let remaining=(preview.until-self.t).ceil().max(0.0) as u32;
        let Some(code)=item.code.as_deref() else{return true;};
        c.text(18.0,25.0,"新验证码",13.0,W::Bold,theme::WHITE,H::Left);
        let close=c.hit(380.0,4.0,28.0,28.0,"mail-preview-close");
        c.text(394.0,23.0,"×",18.0,W::Regular,if close.hovered(){theme::WHITE}else{theme::GRAY},H::Center);
        c.fill(24.0,40.0,164.0,108.0,11.0,theme::CARD);
        c.fill(196.0,40.0,199.0,108.0,11.0,theme::CARD);
        c.text(106.0,59.0,&c.fit(&item.from,11.0,W::Semibold,140.0),11.0,W::Semibold,theme::WHITE,H::Center);
        let size=32.0_f32.min(32.0*140.0/c.text_w(code,32.0,W::Bold).max(1.0));
        c.text(106.0,101.0,code,size,W::Bold,theme::GREEN,H::Center);
        c.text(106.0,130.0,"点击复制",11.0,W::Regular,theme::GRAY,H::Center);
        let copy=c.hit(24.0,40.0,164.0,108.0,("mail-preview-copy",&item.account,item.uid));
        c.text(208.0,60.0,&c.fit(&item.subject,12.0,W::Semibold,175.0),12.0,W::Semibold,theme::WHITE,H::Left);
        let excerpt:String=item.summary.chars().take(400).map(|ch|if ch.is_whitespace(){' '}else{ch}).collect();
        let excerpt=if excerpt.trim().is_empty(){"（这封邮件没有正文）".into()}else{excerpt};
        let mut job=egui::text::LayoutJob::simple(excerpt,c.font(11.5,W::Regular),Color32::from_gray(200),175.0*c.s);
        job.wrap.max_rows=4;
        job.wrap.break_anywhere=true;
        let galley=c.painter.layout_job(job);
        c.painter.galley(c.pos(208.0,73.0),galley,Color32::from_gray(200));
        c.text(24.0,166.0,&format!("{remaining} 秒后收起"),10.5,W::Regular,theme::GRAY,H::Left);
        let read=c.hit(331.0,151.0,64.0,22.0,"mail-preview-read");
        c.text(395.0,166.0,"查看邮件",10.5,W::Regular,theme::BLUE,H::Right);
        if copy.clicked() {cmds.push(Cmd::Copy(code.to_owned()));self.code_copied(code);}
        if close.clicked() {self.end_mail_preview(true);}
        else if read.clicked() {
            self.show_page(5);
            self.mail.open_uid=Some((item.account,item.uid));
        }
        true
    }
}
