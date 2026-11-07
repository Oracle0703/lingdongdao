#![cfg_attr(windows, windows_subsystem = "windows")]
//! Windows 外壳：无边框透明窗口 + 托盘 + 系统数据，界面全部交给 `daozai::app::App`。

use eframe::egui::{self, Color32, Pos2};
use daozai::app::{self, App, Cmd, Env, Options};
use daozai::smtc;
use daozai::poll_clock::PollClock;
use std::time::{Duration, Instant};

mod diagnostics;

struct Shell {
    app: App,
    t0: Instant,
    #[cfg(windows)]
    frame_at: Instant,
    media: smtc::Shared,
    media_sources: smtc::Sources,
    seek_rx: Option<std::sync::mpsc::Receiver<bool>>,
    media_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    placed: bool,
    placement_at: PollClock,
    placement_scale: f32,
    /// 岛窗口按哪个每点像素数设置过尺寸；不一致时（改了岛的大小或换了屏幕缩放）重设窗口大小。
    #[cfg(windows)]
    sized_ppp: f32,
    /// 当前登记给岛窗口的额外放大倍数（设置 → 岛的大小）。
    #[cfg(windows)]
    island_scale: f32,
    win_x: f32,
    sys: (Option<u8>, bool, PollClock),
    #[cfg(windows)]
    system_state: daozai::system::Snapshot,
    #[cfg(windows)]
    system_ready: bool,
    #[cfg(windows)]
    system_rx: std::sync::mpsc::Receiver<daozai::system::Observation>,
    fullscreen: Option<daozai::display_layout::Bounds>,
    input_state:daozai::input::Monitor,
    #[cfg(windows)]
    input_keys:Option<daozai::input::Shortcuts>,
    #[cfg(windows)]
    incoming: Option<std::sync::mpsc::Receiver<daozai::system::Incoming>>,
    #[cfg(windows)]
    hidden: bool,
    #[cfg(windows)]
    native: Option<daozai::native::Window>,
    #[cfg(windows)]
    tray: Option<daozai::tray::Tray>,
    #[cfg(windows)]
    screenshot: Option<daozai::screenshot::Capture>,
    #[cfg(windows)]
    screenshot_notices: Vec<(String, String)>,
    #[cfg(windows)]
    screenshot_hotkey_seen: bool,
    #[cfg(windows)]
    autostart: bool,
    /// v0.5.4：蓝牙音频设备（后台线程推送）与音量回声的静默窗口。
    #[cfg(windows)]
    bt_rx: Option<daozai::bt::Service>,
    bt: Vec<daozai::bt::Device>,
    bt_ready: bool,
    volume_quiet: PollClock,
    /// v0.5.5：邮件收信服务（工作线程），以及上次同步给它的账号/开关快照。
    mail: daozai::mail::Service,
    mail_sig: (bool, u32, Vec<daozai::mail::Account>),
    /// v0.5.6：日程（CalDAV）服务与上次下发的配置。
    cal: daozai::calendar::Service,
    cal_sig: (Vec<daozai::calendar::Config>, bool),
    quota: daozai::quota::Service,
    quota_sig: Option<(bool,Vec<daozai::quota::Account>)>,
    quota_at: PollClock,
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        diagnostics::graphics(cc);
        diagnostics::startup("加载本地配置");
        let mut app = App::new(&cc.egui_ctx, Options::default());
        // 岛的大小只由设置决定：关掉 egui 自带的 Ctrl +/-/0 缩放（它会同时缩放岛和所有窗口）。
        cc.egui_ctx.options_mut(|o|o.zoom_with_keyboard=false);
        #[cfg(feature="record")] let quota_demo=std::env::var("DAOZAI_QUOTA_DEMO").is_ok();
        #[cfg(not(feature="record"))] let quota_demo=false;
        diagnostics::startup("初始化额度配置");
        if !quota_demo{app.quota_initialize();}
        if std::env::args().any(|arg|arg=="--settings"){app.open_settings();}
        if std::env::args().any(|arg|arg=="--calendar"){app.open_calendar(None);}
        #[cfg(feature="record")]
        if std::env::var("DAOZAI_MAIL_DEMO").is_ok(){app.mail_demo();}
        #[cfg(feature="record")]
        if std::env::var("DAOZAI_CAL_DEMO").is_ok(){app.calendar_demo();}
        #[cfg(feature="record")]
        if let Ok(mode)=std::env::var("DAOZAI_QUOTA_DEMO"){app.quota_demo(&mode);}
        diagnostics::startup("初始化媒体和托盘");
        let media = smtc::shared();
        let media_sources=std::sync::Arc::new(std::sync::Mutex::new(smtc::SourceState{preferred:app.settings.music_source.clone(),..Default::default()}));
        let media_enabled=smtc::spawn_poller_with_sources(media.clone(),cc.egui_ctx.clone(),media_sources.clone());
        media_enabled.store(app.settings.visible_pages[0],std::sync::atomic::Ordering::Relaxed);
        #[cfg(windows)]
        let autostart = daozai::tray::autostart_enabled();
        #[cfg(windows)]
        let tray = daozai::tray::Tray::start(daozai::tray::State {
            pet: app.settings.pet,
            minimal: app.settings.minimal,
            autostart,
        });
        diagnostics::startup("初始化通知监听");
        #[cfg(windows)]
        let incoming=if app.settings.read_notifications {match daozai::system::connect(){Ok(rx)=>Some(rx),Err(e)=>{app.listener_status=format!("Windows 无法读取通知：{}",e.code());None}}}else{None};
        // 音频、网络与输入法查询由后台完成，避免首帧等待系统服务。
        diagnostics::startup("创建后台采样与窗口");
        let initial_input=None;
        #[cfg(windows)] let system_state=daozai::system::Snapshot::default();
        #[cfg(windows)] let system_rx=daozai::system::observe(cc.egui_ctx.clone(),system_state.clone(),initial_input);
        // 第一帧之前就登记岛的放大倍数，避免先以原大小显示再跳变。屏幕上限在拿到显示器信息后再校正。
        #[cfg(windows)]
        let native = daozai::native::Window::new(cc);
        #[cfg(windows)]
        let island_scale = native.as_ref().map_or(1.0, |window| {
            let scale=daozai::display_layout::island_zoom(app.settings.ui_scale,None,window.dpi_scale());
            egui_winit::set_window_extra_scale(window.hwnd().0 as isize,scale);
            scale
        });
        Self {
            app,
            t0: Instant::now(),
            #[cfg(windows)]
            frame_at: Instant::now(),
            media,
            media_sources,
            seek_rx: None,
            media_enabled,
            placed: false,
            placement_at: PollClock::default(),
            placement_scale: 0.0,
            #[cfg(windows)]
            sized_ppp: 0.0,
            #[cfg(windows)]
            island_scale,
            win_x: 0.0,
            sys: (None, false, PollClock::default()),
            #[cfg(windows)]
            system_state,
            #[cfg(windows)] system_ready:false,
            #[cfg(windows)] system_rx,
            fullscreen:None,
            input_state:daozai::input::Monitor::new(initial_input),
            #[cfg(windows)] input_keys:daozai::input::Shortcuts::start(),
            #[cfg(windows)] incoming,
            #[cfg(windows)] hidden:false,
            #[cfg(windows)]
            native,
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            screenshot: None,
            #[cfg(windows)]
            screenshot_notices: Vec::new(),
            #[cfg(windows)]
            screenshot_hotkey_seen: false,
            #[cfg(windows)]
            autostart,
            #[cfg(windows)]
            bt_rx: None,
            bt: Vec::new(),
            bt_ready: false,
            volume_quiet: PollClock::default(),
            mail: daozai::mail::Service::start(),
            mail_sig: (false, 0, Vec::new()),
            cal: daozai::calendar::Service::start(),
            cal_sig: (Vec::new(), true),
            quota: daozai::quota::Service::start(), quota_sig: None, quota_at: PollClock::default(),
        }
    }

    #[cfg(windows)]
    fn poll_tray(&mut self, ctx: &egui::Context) {
        use daozai::tray::{self, Event};
        let events = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for e in events {
            match e {
                Event::StartScreenshot => self.begin_screenshot(ctx),
                Event::ScreenshotHotkeyChanged(status) => {
                    if self.screenshot_hotkey_seen || status.error_code.is_some() {
                        self.screenshot_notices.push(("截图快捷键".into(), status.message()));
                    }
                    self.screenshot_hotkey_seen = true;
                }
                Event::OpenSettings=>self.app.open_settings(),
                Event::OpenCalendar=>self.app.open_calendar(None),
                Event::TogglePet => {
                    self.app.set_pet(!self.app.settings.pet);
                    self.app.say(if self.app.settings.pet { "吉祥物已打开" } else { "吉祥物已关闭" });
                }
                Event::ToggleMinimal => {
                    self.app.set_minimal(!self.app.settings.minimal);
                    self.app.say(if self.app.settings.minimal { "极简刘海已打开" } else { "极简刘海已关闭" });
                }
                Event::ToggleAutostart => {
                    let want = !self.autostart;
                    match tray::set_autostart(want) {
                        Ok(()) => {
                            self.autostart = want;
                            self.app.say(if want { "已设为开机启动" } else { "已取消开机启动" });
                        }
                        Err(e) => self.app.say(e),
                    }
                }
            }
        }
        if let Some(t) = &self.tray {
            t.sync(tray::State { pet: self.app.settings.pet, minimal: self.app.settings.minimal, autostart: self.autostart });
        }
    }

    #[cfg(windows)]
    fn begin_screenshot(&mut self, ctx: &egui::Context) {
        if self.app.is_screenshot_capturing() { return; }
        let Some(hwnd) = self.native.as_ref().map(|window| window.hwnd()) else {
            self.screenshot_notices.push(("无法开始截图".into(), "岛窗口尚未就绪，请稍后重试".into()));
            return;
        };
        if !self.app.begin_screenshot() { return; }
        if let Some(native) = &mut self.native { native.set_screenshot_capturing(true); }
        let result = (|| {
            if self.screenshot.is_none() {
                self.screenshot = Some(daozai::screenshot::Capture::new(ctx.clone())?);
            }
            let root=daozai::screenshot::store::resolve_root(&self.app.settings.screenshot_dir);
            self.screenshot.as_mut().unwrap().begin(hwnd,root)
        })();
        if let Err(error) = result {
            self.end_screenshot();
            self.screenshot_notices.push(("无法开始截图".into(), error));
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn end_screenshot(&mut self) {
        self.app.end_screenshot();
        if let Some(native) = &mut self.native { native.set_screenshot_capturing(false); }
    }

    /// 岛的大小：只给岛窗口登记额外放大倍数（vendor/egui-winit 的岛仔补丁），设置、邮件等窗口保持系统缩放。
    /// 岛窗口的每点像素数随之变化，下一帧生效；生效后按画布逻辑尺寸重设窗口，定位逻辑也因此重新居中。
    #[cfg(windows)]
    fn apply_island_scale(&mut self, ctx: &egui::Context) {
        let ppp=ctx.pixels_per_point();
        if (ppp-self.sized_ppp).abs()>1e-3 {
            self.sized_ppp=ppp;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(app::CANVAS_W,app::WIN_H)));
        }
        if self.app.is_screenshot_capturing() {return;}
        let Some(window)=&self.native else{return};
        let system=window.dpi_scale();
        let bounds=daozai::display_layout::selected(&self.app.displays,&self.app.settings.display_id).map(|d|d.bounds);
        self.app.ui_scale_max=(daozai::display_layout::island_zoom(150,bounds,system)*100.0).round() as u16;
        let scale=daozai::display_layout::island_zoom(self.app.settings.ui_scale,bounds,system);
        if (scale-self.island_scale).abs()>1e-3 {
            self.island_scale=scale;
            egui_winit::set_window_extra_scale(window.hwnd().0 as isize,scale);
            ctx.request_repaint();
        }
    }

    #[cfg(windows)]
    fn poll_screenshot(&mut self) {
        use daozai::screenshot::CaptureEvent;
        let events = self.screenshot.as_mut().map(|capture| capture.poll()).unwrap_or_default();
        for event in events {
            match event {
                CaptureEvent::SelectionFinished | CaptureEvent::Cancelled => self.end_screenshot(),
                CaptureEvent::Failed(error) => {
                    self.end_screenshot();
                    self.screenshot_notices.push(("截图失败".into(), error));
                }
                CaptureEvent::Saved { path, width, height, elapsed_ms } => {
                    self.screenshot_notices.push(("截图已保存".into(), format!(
                        "{width} × {height} 像素 · {elapsed_ms:.1} ms\n{}", path.display()
                    )));
                }
                CaptureEvent::SaveFailed(error) => {
                    self.screenshot_notices.push(("截图保存失败".into(), error));
                }
                CaptureEvent::Copied(result) => {
                    let (title, body) = match result {
                        Ok(()) => ("截图已复制", "已复制到剪贴板，可以直接粘贴图片".to_string()),
                        Err(error) => ("截图复制失败", error),
                    };
                    self.screenshot_notices.push((title.into(), body));
                }
            }
        }
    }

    #[cfg(windows)]
    fn show_screenshot_notices(&mut self) {
        if self.app.is_screenshot_capturing() { return; }
        for (title, body) in self.screenshot_notices.drain(..) {
            self.app.say(&body);
            if let Some(tray) = &self.tray { tray.notify(&title, &body, false); }
        }
    }

    fn env(&mut self, ctx: &egui::Context) -> Env {
        let (clock, clock_ms, today, date) = local_now();
        // 电量变化很慢，每 5 秒读一次就够
        if self.sys.2.ready(Duration::from_secs(5)) {
            let (b, c) = read_power();
            if self.app.settings.show_battery && self.sys.0.is_some() && self.sys.1!=c && self.app.settings.system_hud {self.app.alert(if c {"电源已连接"}else{"已切换电池供电"},b.map(|v|format!("当前电量 {v}%")).unwrap_or_else(||"桌面电源状态已更新".into()),5,false);}
            if self.app.settings.show_battery && b.is_some_and(|v|v<=20) && !c && self.sys.0.is_some_and(|v|v>20) {self.app.alert("电量偏低",format!("剩余 {}% · 请连接电源",b.unwrap()),5,true);}
            self.sys = (b, c, PollClock::now());
        }
        let ppp = ctx.pixels_per_point();
        let origin = ctx.input(|i| i.viewport().outer_rect).map(|r| r.min).unwrap_or(Pos2::new(self.win_x, 0.0));
        #[cfg(windows)]
        let origin=self.native.as_ref().and_then(|w|w.logical_origin(ppp)).unwrap_or(origin);
        let cursor = if self.app.is_screenshot_capturing() { None } else {
            cursor_logical(ppp).map(|(x, y)| Pos2::new(x - origin.x, y - origin.y))
        };
        Env {
            t: self.t0.elapsed().as_secs_f64(),
            clock,
            clock_ms,
            today,
            date,
            battery: self.sys.0,
            charging: self.sys.1,
            network: Some(self.system_state.network),
            cursor,
            idle_seconds: {
                #[cfg(windows)] {Some(daozai::system::idle_seconds())}
                #[cfg(not(windows))] {None}
            },
            browser_at_top: {
                #[cfg(windows)] {if self.app.settings.browser_minimal {self.native.as_mut().and_then(|native|native.browser_at_top(&self.app.settings))}else{None}}
                #[cfg(not(windows))] {None}
            },
            media: self.media.lock().map(|m| m.clone()).unwrap_or_default(),
            media_sources: self.media_sources.lock().map(|sources|sources.available.clone()).unwrap_or_default(),
            volume: self.system_state.volume,
            bt: self.bt.clone(),
            bt_ready: self.bt_ready,
        }
    }
}

impl eframe::App for Shell {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.media_enabled.store(self.app.settings.visible_pages[0],std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut sources)=self.media_sources.lock(){sources.preferred.clone_from(&self.app.settings.music_source);}
        #[cfg(windows)]
        {
            // 用帧间隔限制刷新，绕过部分 Intel 驱动在垂直同步中的忙等待。
            let delay=Duration::from_secs_f64(1.0/60.0).saturating_sub(self.frame_at.elapsed());
            if !delay.is_zero(){std::thread::sleep(delay);}
            self.frame_at=Instant::now();
        }
        #[cfg(windows)]
        {
            // 先消费遮罩关闭/归还焦点产生的事件，再解除截图状态。
            // 否则 end_screenshot 刚恢复的悬停会被这条旧通知重新锁住。
            if daozai::native::take_foreground_change() && !self.app.is_screenshot_capturing() {
                self.app.foreground_changed();
            }
            self.poll_screenshot();
            self.poll_tray(ctx);
            self.show_screenshot_notices();
        }
        #[cfg(windows)]
        self.apply_island_scale(ctx);
        #[cfg(windows)]
        if !self.app.is_screenshot_capturing() && (!self.placed || self.placement_scale!=ctx.pixels_per_point() || self.placement_at.ready(Duration::from_secs(1))) {
            self.placement_at=PollClock::now();
            self.placement_scale=ctx.pixels_per_point();
            self.app.displays=daozai::display_layout::connected();
            let display=daozai::display_layout::selected(&self.app.displays,&self.app.settings.display_id);
            if display.is_some_and(|d|self.native.as_ref().is_some_and(|w|w.place_on_display(self.placement_scale,d.bounds))) {
                if !self.placed {diagnostics::startup("目标屏幕物理坐标定位完成，已请求显示");}
                self.placed=true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            } else if self.t0.elapsed()>=Duration::from_secs(2) {
                // Do not leave a hidden startup window forever when monitor discovery fails.
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
        }
        #[cfg(not(windows))]
        if !self.placed {
            if let Some(mon)=ctx.input(|i|i.viewport().monitor_size) {
                self.placed=true;self.win_x=((mon.x-app::WIN_W)/2.0).round();
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(self.win_x,0.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
        }
        #[cfg(windows)]
        {
            while let Ok(observation)=self.system_rx.try_recv(){
                let next=observation.input;
                let manual=next.is_some_and(|state|self.input_keys.as_ref().is_some_and(|keys|keys.allows(state.context)));
                if let Some(label)=self.input_state.observe(next,manual){
                    if let Some(keys)=&self.input_keys{keys.clear();}
                    if self.app.settings.system_hud{self.app.alert("键盘输入语言",label,5,false);}
                }
                let new=observation.system;
                // 切换输出设备时，系统会恢复该设备的音量；等待它稳定后再提示调节。
                if new.audio_endpoint!=self.system_state.audio_endpoint {self.volume_quiet=PollClock::now();}
                if self.system_ready && self.app.settings.system_hud {
                    for (title,body) in new.changes(&self.system_state) {
                        // 拖动音量条刚改过音量时，系统回声不再单独弹一条。
                        if title=="系统音量" && !self.volume_quiet.ready(Duration::from_millis(900)) { continue; }
                        self.app.alert(title,body,5,false);
                    }
                }
                self.system_state=new;
                self.system_ready=true;
                self.fullscreen=observation.fullscreen;
            }
            self.hidden=daozai::display_layout::hide_for_fullscreen(self.app.settings.hide_fullscreen,self.fullscreen,daozai::display_layout::selected(&self.app.displays,&self.app.settings.display_id).map(|d|d.bounds));
            if self.app.settings.bluetooth_notifications {
                if self.bt_rx.is_none() {self.bt_rx=Some(daozai::bt::spawn());}
            } else {
                self.bt_rx=None;
                self.bt.clear();
                self.bt_ready=false;
            }
            if let Some(service)=&self.bt_rx {
                while let Ok(found)=service.rx.try_recv(){ self.bt=found; self.bt_ready=true; }
            }
            if let Some(rx)=&self.incoming {
                while let Ok(event)=rx.try_recv(){match event {daozai::system::Incoming::Revoked=>{self.app.settings.read_notifications=false;if let Err(error)=self.app.settings.save(){self.app.listener_status=format!("通知权限已撤销，但设置保存失败：{error}");}},daozai::system::Incoming::Status(s)=>self.app.listener_status=s,daozai::system::Incoming::Message(a,id,b)=>self.app.receive_notification(a,id,b)}}
            }
        }
        if let Some(rx)=&self.seek_rx {
            if let Ok(success)=rx.try_recv(){
                self.seek_rx=None;
                if !success{self.app.say("播放器未完成跳转，请打开客户端后重试");}
            }
        }
        let env = self.env(ctx);
        // v0.5.5 邮件：开关 / 间隔 / 账号清单变了就重新对齐收信线程，再把事件交给界面。
        let sig=(self.app.settings.mail_enabled,self.app.settings.mail_interval,self.app.mail_accounts().to_vec());
        if sig!=self.mail_sig{
            let accounts:&[daozai::mail::Account]=if sig.0{ &sig.2 }else{ &[] };
            self.mail.sync(accounts,sig.1);
            self.mail_sig=sig;
        }
        while let Some(ev)=self.mail.try_recv(){self.app.mail_event_at(ev,env.t);}
        // 日程连接或自动同步开关变化时下发给工作线程。
        // 录制/截图演示用的是假账号，别让真服务去连（会立刻失败、把状态改成红色）。
        #[cfg(feature="record")]
        let cal_demo=std::env::var("DAOZAI_CAL_DEMO").is_ok();
        #[cfg(not(feature="record"))]
        let cal_demo=false;
        let cal=if cal_demo{Vec::new()}else{self.app.calendar_configs()};
        let auto_sync=self.app.settings.calendar_auto_sync;
        if (&cal,auto_sync)!=(&self.cal_sig.0,self.cal_sig.1){
            self.cal.configure(cal.clone(),auto_sync);
            self.cal_sig=(cal,auto_sync);
        }
        while let Some(ev)=self.cal.try_recv(){self.app.calendar_event(ev);}
        #[cfg(feature="record")] let quota_demo=std::env::var("DAOZAI_QUOTA_DEMO").is_ok();
        #[cfg(not(feature="record"))] let quota_demo=false;
        if !quota_demo {
            let config=self.app.quota_config();let sig=(config.enabled,config.accounts.iter().cloned().map(|mut a|{a.alias.clear();a.pinned=false;a}).collect::<Vec<_>>());
            if self.quota_sig.as_ref()!=Some(&sig) || self.quota_at.ready(Duration::from_secs(180)){
                let accounts:Vec<_>=sig.1.iter().filter(|a|sig.0 && a.enabled).cloned().collect();
                if self.quota.refresh(accounts.clone()){self.app.quota_pending(&accounts);self.quota_sig=Some(sig);self.quota_at=PollClock::now();}
            }
            while let Some(ev)=self.quota.try_recv(){self.app.quota_event(ev);}
        }
        let mut out = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let win = egui::Rect::from_min_size(ui.max_rect().min,egui::vec2(app::WIN_W,app::WIN_H));
                out = Some(self.app.frame(ui, win, &env));
            });
        let Some(out) = out else { return };

        #[cfg(windows)]
        if let Some(native) = &mut self.native {
            let r = |r: egui::Rect| (r.left(), r.top(), r.width(), r.height());
            // 保留主窗口的事件循环，避免全屏隐藏后设置窗口无法关闭或重新打开。
            let region=if self.hidden {(0.0,0.0,0.0,0.0)} else {r(out.region)};
            let hit=if self.hidden {(0.0,0.0,0.0,0.0)} else {r(out.hit)};
            native.update(region, hit, if self.hidden{None}else{out.notice_hit.map(r)},if self.hidden{None}else{out.file_drop_zone.map(r)},ctx.pixels_per_point(), out.editing, !self.hidden && out.expanded);
        }
        for cmd in out.cmds {
            match cmd {
                Cmd::Media(t) => {
                    let Some(source)=self.app.media_control_source().map(str::to_owned) else{continue;};
                    if let smtc::Transport::Seek(seconds)=t {
                        let (tx,rx)=std::sync::mpsc::channel();self.seek_rx=Some(rx);
                        let ctx=ctx.clone();
                        std::thread::spawn(move || {let _=tx.send(smtc::seek(seconds,&source));ctx.request_repaint();});
                    }else{std::thread::spawn(move || smtc::control(t,&source));}
                }
                Cmd::Open(p) => {if let Err(e)=open_path(&p){self.app.say(e);}},
                Cmd::OpenUrl(url)=>{
                    if let Some(url)=daozai::mail::web_url(&url) {
                        if open_path(std::path::Path::new(&url)).is_err(){self.app.say("无法打开默认浏览器，请检查 Windows 默认应用设置");}
                    }
                },
                Cmd::AddFiles=>{
                    #[cfg(windows)] match daozai::system::pick_files(){Ok(files)=>if !files.is_empty(){self.app.on_drop(files);self.app.show_page(3);},Err(e)=>self.app.say(format!("无法选择文件：{}",e.code()))}
                },
                Cmd::DragFile(p)=>{
                    #[cfg(windows)] if let Err(e)=daozai::system::drag_file(&p){self.app.say(format!("无法拖出文件：{}",e.code()));}
                    ctx.input_mut(|i| i.pointer=Default::default());
                },
                Cmd::DisconnectNotifications=>{#[cfg(windows)]{self.incoming=None;self.app.settings.read_notifications=false;self.app.listener_status=match self.app.settings.save(){Ok(())=>"已断开，不再读取 Windows 通知".into(),Err(error)=>format!("本次运行已断开通知，但设置保存失败：{error}")};}},
                Cmd::Reveal(p)=>{let _=std::process::Command::new("explorer.exe").arg(format!("/select,{}",p.display())).spawn();},
                Cmd::QuotaChooseFile=>{
                    #[cfg(windows)] match daozai::system::pick_files_for("选择 AI 账号的登录 JSON 文件",false){Ok(files)=>if let Some(path)=files.into_iter().next(){self.app.quota_set_source(path);},Err(_)=>self.app.say("无法选择登录文件")}
                },
                Cmd::QuotaRefresh=>{
                    if self.quota_at.ready(Duration::from_secs(15)) {
                        let config=self.app.quota_config();let accounts:Vec<_>=config.accounts.iter().filter(|a|config.enabled && a.enabled).cloned().collect();
                        if self.quota.refresh(accounts.clone()){self.app.quota_pending(&accounts);self.quota_at=PollClock::now();}
                    }else{self.app.say("请稍后再刷新");}
                },
                Cmd::QuotaConnected=>{self.quota_sig=None;},
                Cmd::ToggleAutostart=>{
                    #[cfg(windows)] {let want=!self.autostart;match daozai::tray::set_autostart(want){Ok(())=>self.autostart=want,Err(e)=>self.app.say(e)}}
                },
                Cmd::SetVolume(v)=>{
                    #[cfg(windows)] daozai::system::set_volume(v);
                    self.volume_quiet=PollClock::now();
                },
                Cmd::ToggleMute=>{
                    #[cfg(windows)] daozai::system::toggle_mute();
                    self.volume_quiet=PollClock::now();
                },
                Cmd::Copy(text)=>{ctx.copy_text(text);self.app.say("已复制");},
                Cmd::Notify(title,body)=>{
                    #[cfg(windows)] if let Some(tray)=&self.tray{tray.notify(&title,&body,self.app.settings.sound);}
                },
                Cmd::RequestNotifications=>{
                    #[cfg(windows)] {let previous=self.app.settings.clone();self.app.settings.read_notifications=true;if let Err(error)=self.app.settings.save(){self.app.settings=previous;self.app.listener_status=format!("设置保存失败，未连接通知：{error}");}else{self.incoming=None;match daozai::system::connect(){Ok(rx)=>{self.incoming=Some(rx);self.app.listener_status="正在请求 Windows 通知权限…".into();},Err(e)=>self.app.listener_status=format!("Windows 未允许读取通知：{}。需要相应的系统权限或应用身份。",e.code())}}}
                },
                Cmd::MailSync=>{
                    let enabled=self.app.settings.mail_enabled;
                    let accounts=if enabled{self.app.mail_accounts().to_vec()}else{Vec::new()};
                    self.mail.sync(&accounts,self.app.settings.mail_interval);
                    self.mail_sig=(enabled,self.app.settings.mail_interval,self.app.mail_accounts().to_vec());
                },
                Cmd::MailTest(account,password)=>self.mail.test(account,password),
                Cmd::CalendarTest(cfg,password)=>self.cal.test(cfg,password),
                Cmd::CalendarSync=>self.cal.sync_now(),
                Cmd::CalendarMonth(month)=>self.cal.view_month(month),
                Cmd::CalendarForget=>{
                    if let Err(error)=daozai::calendar::forget_config(){self.app.say(&error);continue;}
                    if let Some(old)=self.app.forget_calendar(){daozai::calendar::secrets::delete(&old);}
                    let configs=self.app.calendar_configs();
                    self.cal.configure(configs.clone(),self.app.settings.calendar_auto_sync);
                    self.cal_sig=(configs,self.app.settings.calendar_auto_sync);
                },
                Cmd::Focus => if !self.app.is_screenshot_capturing() { ctx.send_viewport_cmd(egui::ViewportCommand::Focus); },
            }
        }


        #[cfg(windows)] let autostart = self.autostart;
        #[cfg(not(windows))] let autostart = false;
        self.app.settings_ui(ctx, autostart);
        if let Ok(mut sources)=self.media_sources.lock(){sources.preferred.clone_from(&self.app.settings.music_source);}
        self.app.mail_reader_ui(ctx);
        #[cfg(windows)] let repaint_after=if self.hidden{Duration::from_secs(1)}else{out.repaint_after};
        #[cfg(not(windows))] let repaint_after=out.repaint_after;
        schedule_repaint(ctx,repaint_after);
    }
}

// ---------------- 系统信息 ----------------

fn schedule_repaint(ctx:&egui::Context,interval:Duration){
    // egui 会预减一帧；本程序自行调度刷新，补回后才能保持实际的等待间隔。
    let predicted=ctx.input(|i|Duration::from_secs_f32(i.predicted_dt));
    ctx.request_repaint_after(interval.saturating_add(predicted));
}

#[cfg(test)]
mod repaint_tests {
    use super::*;

    fn scheduled_delay(interval:Duration,immediate:bool)->Duration{
        let ctx=egui::Context::default();
        let mut delay=Duration::ZERO;
        for frame in 0..4 {
            let output=ctx.run(egui::RawInput{time:Some(frame as f64),predicted_dt:1.0/60.0,..Default::default()},|ctx|{
                if immediate{ctx.request_repaint();}
                schedule_repaint(ctx,interval);
            });
            delay=output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        delay
    }

    #[test]
    fn background_repaint_keeps_the_requested_frame_interval(){
        for fps in [60.0,30.0,20.0,10.0,1.0] {
            let interval=Duration::from_secs_f64(1.0/fps);
            let actual=scheduled_delay(interval,false);
            assert!(actual.abs_diff(interval)<Duration::from_micros(1),"目标 {fps} 帧不应被提前唤醒：{actual:?}");
        }
    }

    #[test]
    fn interaction_and_service_events_still_repaint_immediately(){
        assert_eq!(scheduled_delay(Duration::from_secs(1),true),Duration::ZERO);
    }
}

#[cfg(windows)]
fn local_now() -> (String, u16, String, String) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    (
        format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond),
        st.wMilliseconds.min(999),
        format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay),
        format!("{}月{}日 {}", st.wMonth, st.wDay, wd[(st.wDayOfWeek as usize).min(6)]),
    )
}

#[cfg(not(windows))]
fn local_now() -> (String, u16, String, String) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    (format!("{h:02}:{m:02}:{s:02}"), now.subsec_millis() as u16, "1970-01-01".into(), "1月1日 周四".into())
}

#[cfg(windows)]
fn read_power() -> (Option<u8>, bool) {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_ok() {
        let pct = if st.BatteryLifePercent == 255 { None } else { Some(st.BatteryLifePercent) };
        (pct, st.ACLineStatus == 1)
    } else {
        (None, false)
    }
}

#[cfg(not(windows))]
fn read_power() -> (Option<u8>, bool) {
    (None, true)
}

#[cfg(windows)]
fn cursor_logical(ppp: f32) -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x as f32 / ppp, p.y as f32 / ppp))
}

#[cfg(not(windows))]
fn cursor_logical(_ppp: f32) -> Option<(f32, f32)> {
    None
}

#[cfg(windows)]
fn open_path(p: &std::path::Path)->Result<(),String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
    wide.push(0);
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    unsafe {
        let result=ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if result.0 as usize<=32 {return Err("无法打开，请检查文件位置和默认应用".into())}
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_path(_p: &std::path::Path)->Result<(),String> {Ok(())}

fn main() -> std::process::ExitCode {
    diagnostics::install();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::report("岛仔启动或运行失败。", &error.to_string());
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eframe::Result<()> {
    // 单实例（与原版同名互斥体）
    #[cfg(windows)]
    if !daozai::tray::single_instance() {
        if !daozai::tray::activate_existing(std::env::args().any(|arg|arg=="--calendar")) {
            diagnostics::report("岛仔已在运行，但暂时无法打开窗口。\n请稍后重试；若仍无法打开，请在任务管理器的“详细信息”中结束 daozai.exe 后重试。", "已有实例可能正在启动或已无响应。请稍后重试；若仍无法打开，请在任务管理器的“详细信息”中结束 daozai.exe，再重新启动。");
        }
        return Ok(());
    }
    diagnostics::startup("初始化图形窗口");
    let viewport = egui::ViewportBuilder::default()
        .with_title("岛仔").with_icon(daozai::theme::app_icon())
        .with_inner_size([app::CANVAS_W, app::WIN_H])
        .with_decorations(false)
        .with_transparent(true)
        .with_active(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_always_on_top()
        .with_visible(false);
    let opts = eframe::NativeOptions { viewport, vsync: !cfg!(windows), ..Default::default() };
    eframe::run_native("岛仔", opts, Box::new(|cc| Ok(Box::new(Shell::new(cc)))))
}
