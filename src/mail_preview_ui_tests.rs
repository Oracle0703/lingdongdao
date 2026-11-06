use super::*;

fn receive(h:&mut H,uid:u32,code:&str) {
    h.app.mail.accounts=vec![crate::mail::Account{email:"preview@example.com".into(),provider:crate::mail::Provider::Qq,custom:None,enabled:true}];
    h.app.mail_event_at(crate::mail::Event::Mail{item:crate::mail::Item{
        account:"preview@example.com".into(),from:"岛仔测试".into(),subject:"确认本次登录".into(),time:"14:00".into(),
        received_unix:Some(chrono::Utc::now().timestamp()),code:Some(code.into()),
        summary:format!("你的验证码是 {code}。本次操作用于验证邮箱登录，请勿将验证码分享给他人。如果不是你本人的操作，请忽略这封邮件。").into(),
        links:vec![],meeting:None,uid},new:true,updates:vec![]},h.t);
}

#[test]
fn mail_preview_opens_without_hover_copies_and_closes_even_with_the_pointer_inside() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.settings.notifications=false;h.app.page=crate::app::SALARY_PAGE;h.frame(vec![]);
    let started=h.t;receive(&mut h,1,"482913");
    for _ in 0..60 {h.frame(vec![]);}
    assert!(h.app.open&&h.cursor.is_none(),"无鼠标悬停也必须自动展开");
    let text_pos=|h:&H,needle:&str|h.shapes.iter().find_map(|s|match &s.shape {
        egui::Shape::Text(t) if t.galley.text().contains(needle)=>Some(t.visual_bounding_rect().center()),_=>None,
    });
    assert!(text_pos(&h,"482913").unwrap().x<text_pos(&h,"本次操作用于").unwrap().x,"左侧验证码、右侧正文简介");
    h.click_text("482913");
    assert!(h.cmds.iter().any(|cmd|matches!(cmd,crate::app::Cmd::Copy(text) if text=="482913")));
    h.t=started+29.9;h.frame(vec![]);assert!(h.app.open);
    h.t=started+30.0;h.frame(vec![]);assert!(!h.app.open&&h.app.mail.preview.is_none());
    for _ in 0..20 {h.frame(vec![]);assert!(!h.app.open,"鼠标未离开时不能立即重新展开");}
    assert_eq!(h.app.page(),crate::app::SALARY_PAGE);
    h.hover(egui::pos2(0.0,230.0),0.3);h.hover(egui::pos2(WIN_W/2.0,12.0),0.7);
    assert!(h.app.open);assert_eq!(h.app.page(),crate::app::SALARY_PAGE,"普通悬停应回到原页面");
}

#[test]
fn mail_preview_close_escape_and_open_mail_keep_normal_navigation_available() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.settings.notifications=false;
    receive(&mut h,1,"482913");for _ in 0..60 {h.frame(vec![]);}
    let close=h.ctx.read_response(egui::Id::new(("card",true,"mail-preview-close"))).expect("应显示关闭按钮");
    h.click(close.rect.center());assert!(!h.app.open&&h.app.mail.preview.is_none());
    receive(&mut h,2,"975310");for _ in 0..60 {h.frame(vec![]);}
    h.cursor=None;h.frame(vec![egui::Event::Key{key:egui::Key::Escape,physical_key:None,pressed:true,repeat:false,modifiers:Default::default()}]);
    assert!(!h.app.open&&h.app.mail.preview.is_none());
    receive(&mut h,3,"654321");for _ in 0..60 {h.frame(vec![]);}
    h.click_text("查看邮件");
    assert!(h.app.mail.preview.is_none());assert_eq!(h.app.page(),5);
    assert_eq!(h.app.mail.open_uid,Some(("preview@example.com".into(),3)));
    h.t+=31.0;h.frame(vec![]);assert!(h.app.open,"主动进入详情后遵循正常悬停逻辑");
}

#[test]
fn mail_preview_yields_to_file_drag_even_when_the_shelf_was_the_previous_page() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.page=3;receive(&mut h,1,"482913");h.frame(vec![]);assert!(h.app.mail.preview.is_some());
    h.hovered_files=vec![egui::HoveredFile{path:Some(std::path::PathBuf::from("preview-test.txt")),..Default::default()}];
    h.frame(vec![]);
    assert!(h.app.mail.preview.is_none()&&h.app.drag_hot&&h.app.open);assert_eq!(h.app.page(),3);
}

#[test]
#[ignore="按需生成验证码自动预览的合成邮件画面"]
fn mail_preview_gallery() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let dest=std::path::PathBuf::from(std::env::var_os("DAOZAI_MAIL_PREVIEW").expect("需指定预览目录"));std::fs::create_dir_all(&dest).unwrap();
    for (label,code,ppp) in [("digits","482913",1.5),("long-code","A9B8C7D6E5F4",2.0)] {
        let mut h=H::new();h.app.settings.pet=false;h.app.settings.notifications=false;receive(&mut h,1,code);
        let mut raster=crate::preview_raster::Raster::new((crate::app::CANVAS_W*ppp)as usize,(WIN_H*ppp)as usize);
        for frame in 0..90 {
            let t=frame as f64/60.0;let env=Env{t,clock:"14:00:00".into(),..Default::default()};
            let raw=egui::RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(crate::app::CANVAS_W,WIN_H))),time:Some(t),viewports:std::iter::once((egui::ViewportId::ROOT,egui::ViewportInfo{native_pixels_per_point:Some(ppp),..Default::default()})).collect(),..Default::default()};
            let out=h.ctx.run(raw,|ctx|{egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{h.app.frame(ui,Rect::from_min_size(Pos2::ZERO,egui::vec2(WIN_W,WIN_H)),&env);});});
            raster.update_textures(&out.textures_delta);
            if frame==89 {
                raster.buf.fill([0.05,0.07,0.10,1.0]);raster.draw(&h.ctx.tessellate(out.shapes,out.pixels_per_point),out.pixels_per_point);
                let mut rgba=Vec::new();raster.to_rgba8(&mut rgba);let img=image::RgbaImage::from_raw((crate::app::CANVAS_W*ppp)as u32,(WIN_H*ppp)as u32,rgba).unwrap();
                image::imageops::crop_imm(&img,((WIN_W-PW)/2.0*ppp-20.0)as u32,0,(PW*ppp+40.0)as u32,(200.0*ppp)as u32).to_image().save(dest.join(format!("{label}.png"))).unwrap();
            }
            raster.free_textures(&out.textures_delta);
        }
    }
}
