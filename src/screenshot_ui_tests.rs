use super::*;
use crate::app::{Cmd, SCREENSHOT_PAGE};
use crate::screenshot::store;

/// 合成一张像截图的图片：浅色背景、标题栏和几块色块；`seed` 区分不同图片。
fn shot(width: u32, height: u32, seed: u8) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let accent = [[62u8, 172, 248], [48, 209, 88], [255, 196, 61], [242, 48, 70]][seed as usize % 4];
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
            let pixel = if fy < 0.08 { [40, 44, 52] }
                else if (0.08..0.11).contains(&fy) { accent }
                else if fx > 0.06 && fx < 0.30 && fy > 0.18 && fy < 0.86 { [226, 230, 238] }
                else if fx > 0.36 && fx < 0.94 && ((fy * 12.0) as u32 % 2 == 0) && fy > 0.2 && fy < 0.8 { [200 - seed * 20, 206, 214] }
                else { [246, 247, 250] };
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
    }
    rgba
}

fn library_with(h: &mut H, shots: &[(u32, u32)]) -> std::path::PathBuf {
    let root = crate::model::config_dir().join("截图");
    for (index, (w, hgt)) in shots.iter().enumerate() {
        let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(9, index as u32, 0, 0).unwrap();
        store::save_png(&shot(*w, *hgt, index as u8), *w, *hgt, &root, time).unwrap();
    }
    h.app.settings.screenshot_dir = root.display().to_string();
    root
}

/// 后台扫描和解码是真实线程：多跑几帧并稍等，直到条件满足。
fn settle(h: &mut H, mut done: impl FnMut(&H) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done(h) {
        assert!(std::time::Instant::now() < deadline, "等待图库加载超时");
        h.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn open_page(h: &mut H) {
    h.app.show_page(SCREENSHOT_PAGE);
    h.hover(H::local(209.5, 100.0), 0.6);
}

fn text_shown(h: &H, needle: &str) -> bool {
    h.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().contains(needle) && t.opacity_factor > 0.9))
}

#[test]
fn page_lists_the_folder_newest_first_and_switches_with_arrows_keys_wheel_and_thumbnails() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    library_with(&mut h, &[(800, 450), (450, 800), (1200, 300)]);
    open_page(&mut h);
    settle(&mut h, |h| h.app.shots.entries.len() == 3 && h.app.shots.loaded() >= 3);
    h.frame(vec![]);
    let names: Vec<_> = h.app.shots.entries.iter().map(|e| e.path.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(names, ["截图_20261108_090200_000.png", "截图_20261108_090100_000.png", "截图_20261108_090000_000.png"]);
    assert!(text_shown(&h, "1 / 3"));
    // 右箭头（悬停预览区时出现）
    h.click(H::local(385.0, 103.0));
    assert_eq!(h.app.shots.selected, 1);
    // 方向键在截图页切图，不翻页
    h.frame(vec![egui::Event::Key { key: egui::Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    assert_eq!((h.app.shots.selected, h.app.page), (2, SCREENSHOT_PAGE));
    h.frame(vec![egui::Event::Key { key: egui::Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    assert_eq!((h.app.shots.selected, h.app.page), (2, SCREENSHOT_PAGE), "到头不跳到其他功能页");
    // 内容区滚轮切图（向上更新）；标题行滚轮仍翻页
    h.hover(H::local(209.5, 100.0), 0.3);
    h.wheel(40.0);
    assert_eq!((h.app.shots.selected, h.app.page), (1, SCREENSHOT_PAGE));
    // 缩略图：选中项居中，左边一格是更新的一张
    h.hover(H::local(209.5, 100.0), 0.3);
    h.click(H::local(209.5 - 52.0, 188.0));
    assert_eq!(h.app.shots.selected, 0);
    h.hover(H::local(210.0, 18.0), 0.3);
    h.wheel(-40.0);
    assert_ne!(h.app.page, SCREENSHOT_PAGE, "标题行滚轮翻页");
}

#[test]
fn actions_emit_open_copy_reveal_and_delete_only_after_confirmation() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    library_with(&mut h, &[(800, 450), (640, 480)]);
    open_page(&mut h);
    settle(&mut h, |h| h.app.shots.entries.len() == 2 && h.app.shots.loaded() >= 2);
    let current = h.app.shots.entries[0].path.clone();
    for label in ["查看原图", "复制", "打开文件夹"] { h.hover(H::local(209.5, 100.0), 0.2); h.click_text(label); }
    assert!(h.cmds.contains(&Cmd::Open(current.clone())));
    assert!(h.cmds.contains(&Cmd::ScreenshotCopy(current.clone())));
    assert!(h.cmds.contains(&Cmd::Reveal(current.clone())));
    h.click_text("删除");
    h.frame(vec![]);
    assert!(!h.cmds.iter().any(|cmd| matches!(cmd, Cmd::ScreenshotDelete(_))), "删除先确认");
    assert!(text_shown(&h, "将此截图移入回收站？"));
    h.click_text("取消");
    h.frame(vec![]);
    assert!(!text_shown(&h, "将此截图移入回收站？"));
    h.click_text("删除");
    h.frame(vec![]);
    h.click_text("移入回收站");
    assert!(h.cmds.contains(&Cmd::ScreenshotDelete(current.clone())));
    h.app.screenshot_deleted(&current, Ok(true));
    assert_eq!(h.app.shots.entries.len(), 1);
}

#[test]
fn saved_screenshot_expands_for_eight_seconds_without_changing_the_page() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    let root = library_with(&mut h, &[(800, 450)]);
    h.app.settings.notifications = false;
    h.app.page = crate::app::SALARY_PAGE;
    h.frame(vec![]);
    let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(10, 0, 0, 0).unwrap();
    let rgba = shot(1000, 600, 3);
    let path = store::save_png(&rgba, 1000, 600, &root, time).unwrap();
    let images = std::sync::Arc::new(crate::screenshot::library::images(1000, 600, &rgba));
    let ctx = h.ctx.clone();
    h.app.screenshot_saved(&ctx, &path, images.clone(), Some(1));
    for _ in 0..30 { h.frame(vec![]); }
    assert!(h.app.open && h.app.screenshot_preview_active(), "无鼠标悬停也自动展开");
    assert!(text_shown(&h, "截图已保存") && text_shown(&h, "正在复制"));
    h.app.screenshot_copied(Ok(()), Some(1));
    h.frame(vec![]);
    assert!(text_shown(&h, "已复制到剪贴板"));
    assert_eq!(h.app.page(), crate::app::SALARY_PAGE, "预览不改变原页面");
    settle(&mut h, |h| h.app.shots.entries.len() == 2);
    assert_eq!(h.app.shots.entries[0].path, path, "新截图排在最前并选中");
    for _ in 0..(8 * 60) { h.frame(vec![]); }
    assert!(!h.app.screenshot_preview_active(), "不悬停 8 秒后收起");
    for _ in 0..30 { h.frame(vec![]); }
    assert!(!h.app.open);

    // 悬停后保持展开，离开后收起
    h.app.screenshot_saved(&ctx, &path, images, Some(2));
    h.hover(H::local(209.5, 100.0), 10.0);
    assert!(h.app.screenshot_preview_active(), "鼠标在岛上时不收起");
    h.hover(Pos2::new(WIN_W / 2.0, 260.0), 0.5);
    assert!(!h.app.screenshot_preview_active(), "离开后收起");
    assert_eq!(h.app.page(), crate::app::SALARY_PAGE);
}

#[test]
fn editing_a_draft_gets_a_toast_instead_of_the_preview() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    let root = library_with(&mut h, &[]);
    h.app.adding = true;
    let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(11, 0, 0, 0).unwrap();
    let rgba = shot(300, 200, 1);
    let path = store::save_png(&rgba, 300, 200, &root, time).unwrap();
    let ctx = h.ctx.clone();
    h.app.screenshot_saved(&ctx, &path, std::sync::Arc::new(crate::screenshot::library::images(300, 200, &rgba)), Some(1));
    assert!(!h.app.screenshot_preview_active() && h.app.adding, "草稿保留，不弹预览");
}

#[test]
fn save_failure_keeps_the_image_and_offers_retry_folder_copy_and_discard() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.settings.notifications = false;
    h.app.page = 0;
    h.frame(vec![]);
    let rgba = shot(800, 450, 0);
    let ctx = h.ctx.clone();
    h.app.screenshot_save_failed(&ctx, "无法创建截图目录 E:/截图：系统找不到指定的路径。".into(), std::sync::Arc::new(crate::screenshot::library::images(800, 450, &rgba)), 1);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(h.app.open && text_shown(&h, "截图未能保存") && text_shown(&h, "系统找不到指定的路径"));
    for (label, cmd) in [("重试", Cmd::ScreenshotRetrySave), ("另选目录", Cmd::ScreenshotChooseFolder), ("复制图片", Cmd::ScreenshotCopyFailed), ("放弃", Cmd::ScreenshotDiscardFailed)] {
        h.click_text(label);
        assert!(h.cmds.contains(&cmd), "{label}");
    }
    // 失败状态比普通的 8 秒多留一会儿
    for _ in 0..(12 * 60) { h.frame(vec![]); }
    assert!(h.app.screenshot_preview_active());
    h.app.clear_screenshot_failure(true);
    assert!(!h.app.screenshot_preview_active(), "放弃后收起");
    h.app.show_screenshot_failure();
    assert!(!h.app.screenshot_preview_active(), "没有待处理的失败时不展开");
}

#[test]
fn copy_result_reaches_the_matching_preview_even_when_it_arrives_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    let root = library_with(&mut h, &[]);
    h.app.settings.notifications = false;
    h.frame(vec![]);
    let ctx = h.ctx.clone();
    let save = |h: &mut H, minute: u32, seq: u64| {
        let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(13, minute, 0, 0).unwrap();
        let rgba = shot(320, 200, minute as u8);
        let path = store::save_png(&rgba, 320, 200, &root, time).unwrap();
        h.app.screenshot_saved(&ctx, &path, std::sync::Arc::new(crate::screenshot::library::images(320, 200, &rgba)), Some(seq));
    };
    // 复制结果先到，“已保存”后到：预览一开始就显示已复制。
    h.app.screenshot_copied(Ok(()), Some(7));
    save(&mut h, 1, 7);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(text_shown(&h, "已复制到剪贴板"));
    // 下一张截图的复制结果不能落到这张的预览上。
    h.app.screenshot_copied(Err("剪贴板被占用".into()), Some(8));
    for _ in 0..30 { h.frame(vec![]); }
    assert!(text_shown(&h, "已复制到剪贴板") && !text_shown(&h, "未能复制"));
    save(&mut h, 2, 8);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(text_shown(&h, "未能复制"), "第二张的预览显示它自己的复制结果");
    // 重试保存（没有序号）不显示复制状态。
    save(&mut h, 3, 0);
    h.app.screenshot_saved(&ctx, &h.app.shots.entries[0].path.clone(), std::sync::Arc::new(crate::screenshot::library::images(2, 2, &[0; 16])), None);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(!text_shown(&h, "正在复制") && !text_shown(&h, "已复制到剪贴板"));
}

#[test]
fn idle_page_does_not_rescan_every_second_but_reentering_does() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    library_with(&mut h, &[(400, 300)]);
    open_page(&mut h);
    settle(&mut h, |h| h.app.shots.entries.len() == 1 && !h.app.shots.is_scanning());
    // 空闲时岛每秒才重绘一次：时间跳过 1.5 秒也不能重新扫描（扫描会清理目录，打扰正在进行的保存）。
    for _ in 0..3 { h.t += 1.5; h.frame(vec![]); assert!(!h.app.shots.is_scanning(), "空闲重绘不应触发扫描"); }
    // 离开再回来：重新扫描。
    h.app.show_page(0);
    for _ in 0..30 { h.frame(vec![]); }
    h.app.show_page(SCREENSHOT_PAGE);
    for _ in 0..40 { h.frame(vec![]); if h.app.shots.is_scanning() { break; } }
    settle(&mut h, |h| !h.app.shots.is_scanning());
}

#[test]
fn ending_the_otp_preview_underneath_keeps_the_screenshot_preview_open() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    let root = library_with(&mut h, &[]);
    h.app.settings.notifications = false;
    h.frame(vec![]);
    let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(14, 0, 0, 0).unwrap();
    let rgba = shot(320, 200, 1);
    let path = store::save_png(&rgba, 320, 200, &root, time).unwrap();
    let ctx = h.ctx.clone();
    h.app.screenshot_saved(&ctx, &path, std::sync::Arc::new(crate::screenshot::library::images(320, 200, &rgba)), Some(1));
    // 截图预览期间收到验证码，然后按 Esc 结束验证码预览（它在截图预览下面）。
    h.app.mail.accounts = vec![crate::mail::Account { email: "preview@example.com".into(), provider: crate::mail::Provider::Qq, custom: None, enabled: true }];
    h.app.mail_event_at(crate::mail::Event::Mail { item: crate::mail::Item {
        account: "preview@example.com".into(), from: "岛仔测试".into(), subject: "确认登录".into(), time: "14:00".into(),
        received_unix: Some(chrono::Utc::now().timestamp()), code: Some("482913".into()), summary: "验证码 482913".into(),
        links: vec![], meeting: None, uid: 1 }, new: true, updates: vec![] }, h.t);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(h.app.mail.preview.is_some(), "验证码预览照常开始");
    h.frame(vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    assert!(h.app.mail.preview.is_none());
    for _ in 0..20 { h.frame(vec![]); }
    assert!(h.app.open && h.app.screenshot_preview_active(), "截图预览还在时不能收起岛");
    assert!((h.app.expanded_height() - crate::screenshot_ui::HEIGHT).abs() < 0.1);
}

#[test]
#[ignore = "按需生成截图页版式图：设置 DAOZAI_SCREENSHOT_PREVIEW 为输出目录"]
fn screenshot_layout_gallery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dest = std::path::PathBuf::from(std::env::var_os("DAOZAI_SCREENSHOT_PREVIEW").expect("需指定预览目录"));
    std::fs::create_dir_all(&dest).unwrap();
    let ppp = 2.0;
    let (cw, ch) = ((crate::app::CANVAS_W * ppp) as usize, (WIN_H * ppp) as usize);
    // 每一帧都把纹理变化交给光栅器，包括等待解码的帧，否则先前上传的截图纹理会缺失。
    fn step(h: &mut H, raster: &mut crate::preview_raster::Raster, ppp: f32, cursor: Option<(f32, f32)>) -> egui::FullOutput {
        h.t += 1.0 / 60.0;
        let env = Env { t: h.t, clock: "12:00:05".into(), today: "2026-11-08".into(), date: "11月8日 周日".into(),
            cursor: Some(cursor.map_or(Pos2::new(WIN_W / 2.0, 12.0), |(x, y)| H::local(x, y))), ..Default::default() };
        let events = cursor.map(|(x, y)| vec![egui::Event::PointerMoved(H::local(x, y))]).unwrap_or_default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(crate::app::CANVAS_W, WIN_H))), time: Some(h.t), events,
            viewports: std::iter::once((egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() })).collect(), ..Default::default() };
        let app = &mut h.app;
        let out = h.ctx.run(raw, |ctx| { egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| { app.frame(ui, Rect::from_min_size(Pos2::ZERO, egui::vec2(WIN_W, WIN_H)), &env); }); });
        raster.update_textures(&out.textures_delta);
        out
    }
    // (名称, 截图尺寸列表, 选中序号, 光标位置, 操作)
    let cases: [(&str, Vec<(u32, u32)>, usize, Option<(f32, f32)>, &str); 8] = [
        ("page-landscape", vec![(1600, 900), (1920, 1080), (900, 1400), (2400, 600), (1280, 800)], 1, Some((209.5, 100.0)), ""),
        ("page-portrait", vec![(1600, 900), (900, 1400), (1280, 800)], 1, None, ""),
        ("page-wide-strip", vec![(3000, 400), (1600, 900)], 0, None, ""),
        ("page-confirm-delete", vec![(1600, 900), (1280, 800)], 0, None, "delete"),
        ("page-empty", vec![], 0, None, ""),
        ("preview-saved", vec![(1600, 900), (1280, 800)], 0, None, "preview-ok"),
        ("preview-copying", vec![(1600, 900)], 0, None, "preview"),
        ("preview-save-failed", vec![], 0, None, "failure"),
    ];
    for (name, shots, selected, cursor, action) in cases {
        let mut h = H::new();
        h.app.settings.pet = false;
        h.app.settings.notifications = false;
        let mut raster = crate::preview_raster::Raster::new(cw, ch);
        let root = library_with(&mut h, &shots);
        if action == "failure" {
            h.app.page = 0;
            step(&mut h, &mut raster, ppp, None);
            let rgba = shot(1440, 900, 3);
            let ctx = h.ctx.clone();
            h.app.screenshot_save_failed(&ctx, "无法创建截图目录 E:/截图：系统找不到指定的路径。".into(), std::sync::Arc::new(crate::screenshot::library::images(1440, 900, &rgba)), 1);
        } else if action.starts_with("preview") {
            h.app.page = 0;
            step(&mut h, &mut raster, ppp, None);
            let time = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_milli_opt(12, 0, 0, 0).unwrap();
            let rgba = shot(1440, 900, 2);
            let saved = store::save_png(&rgba, 1440, 900, &root, time).unwrap();
            let ctx = h.ctx.clone();
            h.app.screenshot_saved(&ctx, &saved, std::sync::Arc::new(crate::screenshot::library::images(1440, 900, &rgba)), Some(1));
            if action == "preview-ok" { h.app.screenshot_copied(Ok(()), Some(1)); }
        } else {
            h.app.show_page(SCREENSHOT_PAGE);
        }
        let expected = shots.len() + usize::from(action.starts_with("preview"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut frames = 0;
        loop {
            let out = step(&mut h, &mut raster, ppp, cursor);
            raster.free_textures(&out.textures_delta);
            frames += 1;
            let ready = h.app.shots.entries.len() >= expected && h.app.shots.loaded() >= h.app.shots.entries.len().min(4);
            // 列表加载完再选中、进入删除确认，然后多跑几帧等邻近的图解码、动画结束。
            if ready && frames < 1000 {
                for _ in 0..selected { h.app.shots.step(1); }
                if action == "delete" { h.app.shots.ask_delete(); }
                frames = 1000;
            }
            if frames > 1060 && ready { break; }
            assert!(std::time::Instant::now() < deadline, "等待图库加载超时：{name}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let out = step(&mut h, &mut raster, ppp, cursor);
        raster.buf.fill([0.93, 0.94, 0.96, 1.0]);
        raster.draw(&h.ctx.tessellate(out.shapes, out.pixels_per_point), out.pixels_per_point);
        let mut rgba = Vec::new(); raster.to_rgba8(&mut rgba);
        let image = image::RgbaImage::from_raw(cw as u32, ch as u32, rgba).unwrap();
        image::imageops::crop_imm(&image, ((WIN_W - PW) / 2.0 * ppp - 20.0) as u32, 0, (PW * ppp + 40.0) as u32, (262.0 * ppp) as u32).to_image().save(dest.join(format!("{name}.png"))).unwrap();
        raster.free_textures(&out.textures_delta);
    }
}

#[test]
#[ignore = "按需生成设置 → 截图的画面：设置 DAOZAI_SCREENSHOT_PREVIEW 为输出目录"]
fn screenshot_settings_gallery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dest = std::path::PathBuf::from(std::env::var_os("DAOZAI_SCREENSHOT_PREVIEW").expect("需指定预览目录"));
    std::fs::create_dir_all(&dest).unwrap();
    for (name, hotkey, label, status) in [("settings-ok", "alt_a", "Alt + A", "Alt + A 已启用。"),
        ("settings-conflict", "alt_a", "", "Alt + A 已被其他程序占用（常见是微信截图），请换一个快捷键，或关掉占用它的程序后重新选择。")] {
        let mut h = H::new();
        h.app.settings.screenshot_hotkey = hotkey.into();
        h.app.set_screenshot_hotkey_status(label, status.into());
        h.app.settings_tab = crate::screenshot_ui::SETTINGS_TAB; h.app.open_settings(); h.ctx.set_embed_viewports(false);
        let mut raster = crate::preview_raster::Raster::new(760, 640);
        for frame in 0..4 {
            let output = std::sync::Arc::new(Mutex::new(None)); let capture = output.clone();
            let time = frame as f64 * 0.02;
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                let id = viewport.ids.this;
                let raw = egui::RawInput { viewport_id: id, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(760.0, 640.0))), time: Some(time), viewports: std::iter::once((id, egui::ViewportInfo::default())).collect(), ..Default::default() };
                *capture.lock().unwrap() = Some(ctx.run(raw, |ctx| (viewport.viewport_ui_cb)(ctx)));
            });
            let parent = h.ctx.run(egui::RawInput::default(), |ctx| h.app.settings_ui(ctx, false));
            let child = output.lock().unwrap().take().unwrap();
            let mut set: Vec<_> = parent.textures_delta.set.iter().chain(&child.textures_delta.set).cloned().collect();
            set.sort_by_key(|(_, delta)| delta.pos.is_some());
            raster.update_textures(&egui::TexturesDelta { set, free: vec![] });
            if frame == 3 {
                raster.buf.fill([0.0, 0.0, 0.0, 1.0]);
                raster.draw(&h.ctx.tessellate(child.shapes, child.pixels_per_point), child.pixels_per_point);
                let mut rgba = Vec::new(); raster.to_rgba8(&mut rgba);
                image::save_buffer(dest.join(format!("{name}.png")), &rgba, 760, 640, image::ColorType::Rgba8).unwrap();
            }
        }
    }
}
