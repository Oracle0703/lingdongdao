//! 设置窗口独立外观，不改变岛体的绘制样式。
use eframe::egui::{self,Color32};

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum SettingsTheme { #[default] System, Light, Dark }
impl SettingsTheme {
    pub fn key(self)->&'static str {match self {Self::System=>"system",Self::Light=>"light",Self::Dark=>"dark"}}
    pub fn parse(value:&str)->Self {match value {"light"=>Self::Light,"dark"=>Self::Dark,_=>Self::System}}
    pub fn dark(self,system_dark:bool)->bool {match self {Self::System=>system_dark,Self::Light=>false,Self::Dark=>true}}
}

pub fn system_dark(ctx:&egui::Context)->bool {
    let id=egui::Id::new("settings-system-theme");
    if let Some((at,dark))=ctx.data(|d|d.get_temp::<(std::time::Instant,bool)>(id)) {
        if at.elapsed()<std::time::Duration::from_secs(1){return dark;}
    }
    #[cfg(windows)]
    let dark={
        use windows::{core::w,Win32::System::Registry::*};
        let mut value=1u32;let mut size=4u32;
        unsafe { let _=RegGetValueW(HKEY_CURRENT_USER,w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),RRF_RT_REG_DWORD,None,Some((&mut value as *mut u32).cast()),Some(&mut size)); }
        value==0
    };
    #[cfg(not(windows))]
    let dark=ctx.input(|i|i.raw.system_theme)==Some(egui::Theme::Dark);
    ctx.data_mut(|d|d.insert_temp(id,(std::time::Instant::now(),dark)));
    dark
}

pub fn color(ctx:&egui::Context,dark:Color32,light:Color32)->Color32 {
    if ctx.style().visuals.dark_mode {dark}else{light}
}

/// 设置窗口共用的控件尺寸、颜色与滚动提示，不影响岛体。
pub(crate) fn apply(style:&mut egui::Style,dark:bool) {
    style.visuals=if dark{egui::Visuals::dark()}else{egui::Visuals::light()};
    let palette=|night,day|if dark{night}else{day};
    style.spacing.item_spacing=egui::vec2(8.0,6.0);
    style.spacing.button_padding=egui::vec2(10.0,6.0);
    style.spacing.scroll=egui::style::ScrollStyle{bar_width:8.0,bar_inner_margin:8.0,handle_min_length:32.0,foreground_color:true,..egui::style::ScrollStyle::solid()};
    style.text_styles.insert(egui::TextStyle::Small,egui::FontId::proportional(12.0));
    let visuals=&mut style.visuals;
    visuals.override_text_color=Some(palette(Color32::from_rgb(230,234,242),Color32::from_rgb(36,46,63)));
    visuals.extreme_bg_color=palette(Color32::from_rgb(27,31,39),Color32::from_rgb(248,250,253));
    visuals.selection.bg_fill=palette(Color32::from_rgb(39,73,119),Color32::from_rgb(221,234,253));
    visuals.selection.stroke=egui::Stroke::new(1.0_f32,palette(Color32::from_rgb(157,195,255),Color32::from_rgb(38,96,190)));
    for widget in [&mut visuals.widgets.inactive,&mut visuals.widgets.hovered,&mut visuals.widgets.active] {
        widget.corner_radius=egui::CornerRadius::same(7);
        widget.bg_stroke=egui::Stroke::new(1.0_f32,palette(Color32::from_rgb(65,74,89),Color32::from_rgb(218,224,234)));
    }
    visuals.widgets.inactive.bg_fill=palette(Color32::from_rgb(43,49,61),Color32::from_rgb(240,244,250));
    visuals.widgets.inactive.weak_bg_fill=visuals.widgets.inactive.bg_fill;
    visuals.widgets.hovered.bg_fill=palette(Color32::from_rgb(54,68,89),Color32::from_rgb(228,237,251));
    visuals.widgets.hovered.weak_bg_fill=visuals.widgets.hovered.bg_fill;
    visuals.widgets.active.bg_fill=palette(Color32::from_rgb(47,76,119),Color32::from_rgb(214,230,253));
    visuals.widgets.active.weak_bg_fill=visuals.widgets.active.bg_fill;
}
