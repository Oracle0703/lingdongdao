//! 沙漏页：翻转开始、时长预设与微调、开始 / 暂停、流完提醒，以及收起时岛上的小沙漏。
use super::*;
use crate::app::HOURGLASS_PAGE;

const GLASS: (f32, f32) = (84.0, 79.0);

fn open_hourglass(h: &mut H) {
    h.app.show_page(HOURGLASS_PAGE);
    h.hover(H::local(300.0, 60.0), 1.0);
    assert!(h.app.open);
    assert_eq!(h.app.page, HOURGLASS_PAGE);
}

fn preset(index: usize) -> Pos2 {
    H::local(147.0 + index as f32 * 39.0 + 17.0, 106.0)
}

#[test]
fn clicking_the_hourglass_flips_it_and_the_sand_runs_out_with_an_alert() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    open_hourglass(&mut h);
    assert_eq!(h.app.settings.hourglass_minutes, 3, "默认 3 分钟");
    assert!(!h.app.hourglass.active, "没开始时沙子都在下半边");
    h.click(H::local(GLASS.0, GLASS.1));
    assert!(h.app.hourglass.running && h.app.hourglass.active, "点沙漏翻转开始");
    h.hover(H::local(GLASS.0, GLASS.1), 0.2);
    h.click(H::local(GLASS.0, GLASS.1));
    assert!(h.app.hourglass.running && h.app.hourglass.remaining > 170.0, "翻转动画期间再点不会翻回去");
    h.hover(H::local(300.0, 60.0), 1.0);
    assert!(h.app.hourglass.remaining < 179.0, "开始后倒计时");
    // 计时中再翻转：上下两边的沙子对调。
    h.app.hourglass.remaining = 120.0;
    h.click(H::local(GLASS.0, GLASS.1));
    assert!((h.app.hourglass.remaining - 60.0).abs() < 0.5, "剩余时间变成已过去的时间：{}", h.app.hourglass.remaining);
    h.app.hourglass.remaining = 0.05;
    h.hover(H::local(300.0, 60.0), 0.2);
    assert!(!h.app.hourglass.running && !h.app.hourglass.active, "流完后回到没开始");
    assert!(h.app.history.iter().any(|alert| alert.title == "沙漏时间到"), "流完时提醒");
    assert!(h.cmds.iter().any(|cmd| matches!(cmd, crate::app::Cmd::Notify(title, _) if title == "沙漏时间到")), "同时发系统通知");
}

#[test]
fn presets_and_minute_buttons_set_the_duration_and_remember_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    open_hourglass(&mut h);
    h.click(preset(2));
    assert_eq!(h.app.settings.hourglass_minutes, 5);
    assert_eq!(h.app.hourglass.total, 300.0);
    h.hover(H::local(300.0, 60.0), 0.3);
    h.click(H::local(324.0, 51.0));
    assert_eq!(h.app.settings.hourglass_minutes, 6, "+ 多 1 分钟");
    h.hover(H::local(300.0, 60.0), 0.3);
    h.click(H::local(204.0, 51.0));
    h.hover(H::local(300.0, 60.0), 0.3);
    h.click(H::local(204.0, 51.0));
    assert_eq!(h.app.settings.hourglass_minutes, 4, "− 少 1 分钟");
    assert_eq!(crate::model::Settings::load().hourglass_minutes, 4, "时长保存到设置，下次打开保持一致");
    // 计时中选预设：停止并换成新的时长。
    h.click(H::local(GLASS.0, GLASS.1));
    assert!(h.app.hourglass.running);
    h.hover(H::local(300.0, 60.0), 0.8);
    h.click(preset(0));
    assert!(!h.app.hourglass.running && !h.app.hourglass.active);
    assert_eq!(h.app.hourglass.total, 60.0);
    // 设置窗口里改时长：没在计时的沙漏立即跟随。
    h.app.settings.hourglass_minutes = 12;
    h.frame(vec![]);
    assert_eq!(h.app.hourglass.total, 720.0);
}

#[test]
fn play_button_starts_pauses_resumes_and_reset_stops() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    open_hourglass(&mut h);
    let play = H::local(363.0, 106.0);
    h.click(play);
    assert!(h.app.hourglass.running, "开始");
    h.hover(H::local(300.0, 60.0), 0.8);
    h.click(play);
    assert!(!h.app.hourglass.running && h.app.hourglass.active, "暂停后沙子停在原处");
    let paused = h.app.hourglass.remaining;
    h.hover(H::local(300.0, 60.0), 0.5);
    assert_eq!(h.app.hourglass.remaining, paused, "暂停时不走");
    h.click(play);
    assert!(h.app.hourglass.running, "继续");
    h.hover(H::local(300.0, 60.0), 0.5);
    h.click(H::local(329.0, 50.0));
    assert!(!h.app.hourglass.running && !h.app.hourglass.active, "重置后回到没开始");
    assert_eq!(h.app.hourglass.remaining, h.app.hourglass.total);
}

#[test]
fn a_running_hourglass_shows_on_the_collapsed_island() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    for _ in 0..30 { h.frame(vec![]); }
    let plain = h.app.current_width();
    open_hourglass(&mut h);
    h.click(H::local(GLASS.0, GLASS.1));
    h.hover(Pos2::new(WIN_W / 2.0, 200.0), 5.0); // show_page 会强制展开 4 秒
    assert!(!h.app.open);
    assert!(h.app.hourglass_on_island());
    assert!(h.app.current_width() > plain + 40.0, "收起时岛上留出小沙漏和剩余时间的位置");
    let texts: Vec<String> = h.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) => Some(t.galley.text().to_string()), _ => None }).collect();
    assert!(texts.iter().any(|text| text.starts_with("02:5")), "岛上显示剩余时间：{texts:?}");
    // 番茄钟也在计时：显示先结束的那个。
    h.app.pomo.running = true;
    h.app.pomo.remaining = 60.0;
    h.frame(vec![]);
    assert!(!h.app.hourglass_on_island(), "番茄钟先结束时显示番茄钟");
    h.app.pomo.remaining = 900.0;
    h.frame(vec![]);
    assert!(h.app.hourglass_on_island(), "沙漏先结束时显示沙漏");
}

/// 渲染沙漏页和收起的岛，供人工查看：`DAOZAI_HOURGLASS_PREVIEW=<目录> cargo test hourglass_gallery -- --ignored`
#[test]
#[ignore]
fn hourglass_gallery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dest = std::path::PathBuf::from(std::env::var_os("DAOZAI_HOURGLASS_PREVIEW").expect("需指定输出目录"));
    std::fs::create_dir_all(&dest).unwrap();
    let ppp = 2.0;
    // 字体纹理只在第一次上传，所以整组共用一个画布。
    let height = 170.0;
    let mut raster = crate::preview_raster::Raster::new((WIN_W * ppp) as usize, (height * ppp) as usize);
    let mut render = |h: &mut H, name: &str, cursor: Option<Pos2>, frames: usize| {
        for frame in 0..frames {
            h.t += 1.0 / 60.0;
            let env = Env { t: h.t, clock: "21:30:00".into(), cursor, ..Default::default() };
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, height))), time: Some(h.t),
                events: cursor.map(|p| vec![egui::Event::PointerMoved(p)]).unwrap_or_default(),
                viewports: std::iter::once((egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() })).collect(), ..Default::default() };
            let app = &mut h.app;
            let out = h.ctx.run(raw, |ctx| { egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| { app.frame(ui, Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, WIN_H)), &env); }); });
            raster.update_textures(&out.textures_delta);
            if frame + 1 == frames {
                raster.buf.fill([0.93, 0.94, 0.96, 1.0]);
                raster.draw(&h.ctx.tessellate(out.shapes, out.pixels_per_point), out.pixels_per_point);
                let mut rgba = Vec::new(); raster.to_rgba8(&mut rgba);
                image::save_buffer(dest.join(format!("{name}.png")), &rgba, (WIN_W * ppp) as u32, (height * ppp) as u32, image::ColorType::Rgba8).unwrap();
            }
            raster.free_textures(&out.textures_delta);
        }
    };
    let inside = Some(H::local(300.0, 60.0));
    let mut h = H::new();
    h.app.settings.pet = false;
    h.app.show_page(HOURGLASS_PAGE);
    render(&mut h, "page-idle", inside, 90);
    h.app.hourglass.flip(h.t);
    render(&mut h, "page-flipping", inside, 18);
    render(&mut h, "page-started", inside, 40);
    h.app.hourglass.remaining = h.app.hourglass.total * 0.4;
    render(&mut h, "page-running-40", inside, 20);
    h.app.hourglass.running = false;
    render(&mut h, "page-paused", inside, 20);
    h.app.hourglass.running = true;
    h.app.hourglass.remaining = h.app.hourglass.total * 0.75;
    render(&mut h, "island", Some(Pos2::new(WIN_W / 2.0, 200.0)), 330);
}
