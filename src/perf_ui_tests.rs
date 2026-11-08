//! 系统监控：收起时只显示 CPU / 内存圆环，展开「系统」页显示百分比、内存用量和实时网速。
use super::*;
use crate::app::SYSTEM_PAGE;
use crate::perf::Sample;

const GB: u64 = 1024 * 1024 * 1024;

fn sample(cpu: f32, memory: f32, down: f64, up: f64) -> Sample {
    Sample { cpu, memory, memory_used: (16.0 * memory as f64 / 100.0 * GB as f64) as u64, memory_total: 16 * GB, down, up }
}

fn texts(h: &H) -> Vec<String> {
    h.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) if t.opacity_factor > 0.5 => Some(t.galley.text().to_string()), _ => None }).collect()
}

#[test]
fn collapsed_island_shows_only_the_rings_and_hovering_them_opens_the_system_page() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.settings.pet = false;
    assert!(h.app.settings.system_island && h.app.perf_wanted(), "默认打开");
    for _ in 0..30 { h.frame(vec![]); }
    let with_rings = h.app.current_width();
    h.app.push_perf(sample(37.0, 62.0, 1.25 * 1024.0 * 1024.0, 512.0 * 1024.0));
    for _ in 0..30 { h.frame(vec![]); }
    let x = h.app.resident.target.x[crate::bar_layout::SYSTEM].expect("岛上有系统监控圆环");
    let shown = texts(&h);
    assert!(!shown.iter().any(|text| text.contains('%') || text.contains("KB") || text.contains("MB")), "收起时只画圆环，不显示数字：{shown:?}");
    h.app.settings.system_island = false;
    for _ in 0..60 { h.frame(vec![]); }
    assert!(h.app.resident.target.x[crate::bar_layout::SYSTEM].is_none());
    assert!(h.app.current_width() < with_rings - 30.0, "关掉后岛变窄（只有时钟时岛不窄于 96）");
    h.app.settings.system_island = true;
    for _ in 0..60 { h.frame(vec![]); }
    h.hover(Pos2::new(WIN_W / 2.0 + x, 15.0), 1.0);
    assert!(h.app.open);
    assert_eq!(h.app.page, SYSTEM_PAGE, "悬停在圆环上展开系统页");
}

#[test]
fn system_page_shows_percentages_memory_and_speeds_in_kb_or_mb() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.show_page(SYSTEM_PAGE);
    h.hover(H::local(300.0, 60.0), 0.5);
    assert!(texts(&h).iter().any(|text| text.starts_with("正在读取")), "还没有数据时提示正在读取");
    for second in 0..5 { h.app.push_perf(sample(30.0 + second as f32, 62.0, 1.25 * 1024.0 * 1024.0, 512.0 * 1024.0)); }
    h.hover(H::local(300.0, 60.0), 0.6);
    let shown = texts(&h);
    for expected in ["34", "62", "%", "CPU", "内存 9.9/16.0 GB", "下载", "上传", "1.25", "MB/s", "512", "KB/s"] {
        assert!(shown.iter().any(|text| text == expected), "展开后显示 {expected}：{shown:?}");
    }
    assert_eq!(h.app.perf.history.len(), 5);
    h.app.push_perf(sample(5.0, 40.0, 900.0 * 1024.0, 3.4 * 1024.0));
    h.hover(H::local(300.0, 60.0), 0.2);
    let shown = texts(&h);
    assert!(shown.iter().any(|text| text == "900") && shown.iter().any(|text| text == "3.4"), "不到 1 MB/s 用 KB/s：{shown:?}");
    assert!(!shown.iter().any(|text| text == "MB/s"));
}

#[test]
fn sampling_stops_and_old_values_are_dropped_when_the_feature_is_off() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.push_perf(sample(50.0, 50.0, 0.0, 0.0));
    h.app.settings.system_island = false;
    h.frame(vec![]);
    assert!(h.app.perf_wanted(), "系统页开着时继续采样，打开页面能看到最近的走势");
    assert!(h.app.perf.latest.is_some());
    h.app.set_page_visible(SYSTEM_PAGE, false);
    h.frame(vec![]);
    assert!(!h.app.perf_wanted());
    assert!(h.app.perf.latest.is_none() && h.app.perf.history.is_empty(), "关掉后不保留过时的数值");
    h.app.settings.save().unwrap();
    let saved = crate::model::Settings::load();
    assert!(!saved.system_island && !saved.visible_pages[SYSTEM_PAGE], "设置保存");
}

/// 渲染系统页与收起的岛，供人工查看：`DAOZAI_PERF_PREVIEW=<目录> cargo test perf_gallery -- --ignored`
#[test]
#[ignore]
fn perf_gallery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dest = std::path::PathBuf::from(std::env::var_os("DAOZAI_PERF_PREVIEW").expect("需指定输出目录"));
    std::fs::create_dir_all(&dest).unwrap();
    let ppp = 2.0;
    let height = 170.0;
    let mut raster = crate::preview_raster::Raster::new((WIN_W * ppp) as usize, (height * ppp) as usize);
    let mut render = |h: &mut H, name: &str, cursor: Option<Pos2>, battery: Option<u8>, frames: usize| {
        for frame in 0..frames {
            h.t += 1.0 / 60.0;
            let env = Env { t: h.t, clock: "21:30:00".into(), cursor, battery, ..Default::default() };
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
    let mut h = H::new();
    h.app.settings.pet = false;
    for second in 0..60 {
        let wave = (second as f64 / 6.0).sin().abs();
        h.app.push_perf(sample(20.0 + 25.0 * wave as f32, 58.0, 200.0 * 1024.0 + 1.6 * 1024.0 * 1024.0 * wave, 40.0 * 1024.0 * (1.0 - wave)));
    }
    h.app.push_perf(sample(37.0, 62.0, 1.25 * 1024.0 * 1024.0, 512.0 * 1024.0));
    let away = Some(Pos2::new(WIN_W / 2.0, 200.0));
    render(&mut h, "island-desktop", away, None, 120);
    render(&mut h, "island-laptop", away, Some(76), 120);
    h.app.push_perf(sample(93.0, 88.0, 12.0 * 1024.0 * 1024.0, 3.4 * 1024.0));
    render(&mut h, "island-busy", away, None, 30);
    h.app.show_page(SYSTEM_PAGE);
    h.app.push_perf(sample(37.0, 62.0, 1.25 * 1024.0 * 1024.0, 512.0 * 1024.0));
    render(&mut h, "page", Some(H::local(300.0, 60.0)), None, 90);
    h.app.push_perf(sample(93.0, 88.0, 12.0 * 1024.0 * 1024.0, 3.4 * 1024.0));
    render(&mut h, "page-busy", Some(H::local(300.0, 60.0)), None, 60);
}
