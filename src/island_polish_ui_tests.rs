//! 岛的交互改进：收起歌词岛上的播放控制、拖动边缘调整大小并记住。
use super::*;
use crate::app::Cmd;
use crate::smtc::Transport;

fn lyric_island(h: &mut H) {
    h.app.settings.island_lyrics = true;
    h.app.settings.pet = false;
    h.media = crate::smtc::NowPlaying { title: "Track".into(), artist: "Artist".into(), playing: true, position: 7.0, duration: 120.0, ..Default::default() };
    h.app.set_lyrics("Track", "Artist", 120.0, crate::lyrics::parse_lrc("[00:05]晚风吹过了山岗\n[00:15]下一句"));
    for _ in 0..90 { h.frame(vec![]); }
}

fn press(h: &mut H, p: Pos2, pressed: bool) {
    h.cursor = Some(p);
    h.frame(vec![egui::Event::PointerMoved(p), egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]);
}

fn drag(h: &mut H, from: Pos2, to: Pos2) {
    h.cursor = Some(from);
    h.frame(vec![egui::Event::PointerMoved(from)]);
    press(h, from, true);
    for step in 1..=8 {
        let p = from + (to - from) * (step as f32 / 8.0);
        h.cursor = Some(p);
        h.frame(vec![egui::Event::PointerMoved(p)]);
    }
    press(h, to, false);
    h.frame(vec![]);
}

#[test]
fn collapsed_lyric_island_has_playback_controls_that_do_not_expand_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    lyric_island(&mut h);
    assert!(!h.app.open);
    let right = WIN_W / 2.0 + h.app.current_width() / 2.0;
    // 悬停在控制按钮上不展开
    h.hover(Pos2::new(right - 40.0, 15.0), 1.0);
    assert!(!h.app.open, "悬停在播放控制上不能展开岛");
    for (dx, expected) in [(-62.0, Transport::Prev), (-40.0, Transport::Toggle), (-18.0, Transport::Next)] {
        h.cmds.clear();
        h.click(Pos2::new(right + dx, 15.0));
        assert!(h.cmds.contains(&Cmd::Media(expected)), "{expected:?}");
        assert!(!h.app.open);
    }
    // 悬停在歌词上照常展开
    h.hover(Pos2::new(WIN_W / 2.0 - 40.0, 15.0), 1.0);
    assert!(h.app.open, "歌词区域悬停仍然展开");
}

#[test]
fn dragging_the_collapsed_pill_edge_widens_it_and_remembers_the_width() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    for _ in 0..60 { h.frame(vec![]); }
    let natural = h.app.current_width();
    let edge = Pos2::new(WIN_W / 2.0 + natural / 2.0 - 2.0, 15.0);
    h.hover(edge, 1.0);
    assert!(!h.app.open, "光标在边缘上不展开，方便拖动");
    drag(&mut h, edge, edge + Vec2::new(40.0, 0.0));
    assert!(!h.app.open);
    let saved = crate::model::Settings::load().pill_width;
    assert!((f32::from(saved) - (natural + 80.0)).abs() <= 1.0, "两侧对称变宽：{natural} → {saved}");
    for _ in 0..30 { h.frame(vec![]); }
    assert!((h.app.current_width() - f32::from(saved)).abs() < 1.0);
    // 拖得比内容还窄：回到自动
    let edge = Pos2::new(WIN_W / 2.0 + h.app.current_width() / 2.0 - 2.0, 15.0);
    drag(&mut h, edge, edge - Vec2::new(200.0, 0.0));
    assert_eq!(crate::model::Settings::load().pill_width, 0, "不能窄于内容，回到自动宽度");
    // 重启后保持
    h.app.settings.pill_width = 260; h.app.settings.save().unwrap();
    let restarted = App::new(&h.ctx, Options { fetch_lyrics: false, pet_idle: 1e9 });
    assert_eq!(restarted.settings.pill_width, 260);
}

#[test]
fn lyric_island_width_drags_separately_and_double_click_restores_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    lyric_island(&mut h);
    let width = h.app.current_width();
    let edge = Pos2::new(WIN_W / 2.0 - width / 2.0 + 2.0, 15.0);
    drag(&mut h, edge, edge - Vec2::new(50.0, 0.0));
    assert_eq!(crate::model::Settings::load().lyric_width, (crate::app::LYRIC_BAR_W + 100.0) as u16, "左边缘向左拖也变宽");
    assert_eq!(h.app.settings.pill_width, 0, "歌词岛与普通药丸分别记住");
    for _ in 0..30 { h.frame(vec![]); }
    let edge = Pos2::new(WIN_W / 2.0 - h.app.current_width() / 2.0 + 2.0, 15.0);
    h.cursor = Some(edge);
    h.frame(vec![egui::Event::PointerMoved(edge)]);
    for _ in 0..2 { press(&mut h, edge, true); press(&mut h, edge, false); }
    h.frame(vec![]);
    assert_eq!(crate::model::Settings::load().lyric_width, 0, "双击边缘恢复默认");
    // 最小宽度
    let edge = Pos2::new(WIN_W / 2.0 + h.app.current_width() / 2.0 - 2.0, 15.0);
    drag(&mut h, edge, edge - Vec2::new(150.0, 0.0));
    assert_eq!(crate::model::Settings::load().lyric_width, crate::model::LYRIC_WIDTH_MIN);
}

#[test]
fn dragging_the_expanded_edge_scales_the_island_and_keeps_it_open() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.hover(Pos2::new(WIN_W / 2.0, 12.0), 1.0);
    h.hover(H::local(300.0, 60.0), 0.8);
    assert!(h.app.open);
    let edge = H::local(PW - 3.0, 80.0);
    // 拖到岛外也保持展开
    drag(&mut h, edge, edge + Vec2::new(60.0, 0.0));
    assert!(h.app.open, "拖动中和刚松开时不收起");
    let scale = crate::model::Settings::load().ui_scale;
    assert!((125..=130).contains(&scale), "向外拖 60 点约放大 29%：{scale}");
    assert_eq!(scale, h.app.settings.ui_scale);
    // 下限 80%
    let edge = H::local(PW - 3.0, 80.0);
    drag(&mut h, edge, edge - Vec2::new(300.0, 0.0));
    assert_eq!(crate::model::Settings::load().ui_scale, 80);
}

#[test]
fn collapsed_widths_are_saved_and_out_of_range_values_fall_back_to_auto() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _h = H::new();
    let directory = crate::model::config_dir();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("settings.txt"), "pill_width=250\nlyric_width=420\n").unwrap();
    let loaded = crate::model::Settings::load();
    assert_eq!((loaded.pill_width, loaded.lyric_width), (250, 420));
    std::fs::write(directory.join("settings.txt"), "pill_width=9000\nlyric_width=100\n").unwrap();
    let loaded = crate::model::Settings::load();
    assert_eq!((loaded.pill_width, loaded.lyric_width), (0, 0));
}

#[test]
#[ignore = "按需生成收起歌词岛的画面：设置 DAOZAI_POLISH_PREVIEW 为输出目录"]
fn collapsed_lyric_island_gallery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dest = std::path::PathBuf::from(std::env::var_os("DAOZAI_POLISH_PREVIEW").expect("需指定输出目录"));
    std::fs::create_dir_all(&dest).unwrap();
    for (name, width, pet) in [("lyric-default", 0u16, false), ("lyric-with-pet", 0, true), ("lyric-widened", 460, false)] {
        let mut h = H::new();
        h.app.settings.lyric_width = width;
        lyric_island(&mut h);
        h.app.settings.pet = pet;
        let ppp = 2.0;
        let mut raster = crate::preview_raster::Raster::new((WIN_W * ppp) as usize, (60.0 * ppp) as usize);
        for frame in 0..80 {
            h.t += 1.0 / 60.0;
            let env = Env { t: h.t, clock: "21:30:00".into(), media: h.media.clone(), cursor: None, ..Default::default() };
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, 60.0))), time: Some(h.t),
                viewports: std::iter::once((egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() })).collect(), ..Default::default() };
            let app = &mut h.app;
            let out = h.ctx.run(raw, |ctx| { egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| { app.frame(ui, Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, WIN_H)), &env); }); });
            raster.update_textures(&out.textures_delta);
            if frame == 79 {
                raster.buf.fill([0.93, 0.94, 0.96, 1.0]);
                raster.draw(&h.ctx.tessellate(out.shapes, out.pixels_per_point), out.pixels_per_point);
                let mut rgba = Vec::new(); raster.to_rgba8(&mut rgba);
                image::save_buffer(dest.join(format!("{name}.png")), &rgba, (WIN_W * ppp) as u32, (60.0 * ppp) as u32, image::ColorType::Rgba8).unwrap();
            }
            raster.free_textures(&out.textures_delta);
        }
    }
}
