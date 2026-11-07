//! 截图：岛上的「截图」页，以及设置窗口里的「截图」分组。
use crate::app::{App, Cmd, Env, SCREENSHOT_PAGE};
use crate::extras::{setting_row, settings_card, settings_control_row, settings_label};
use crate::model::SCREENSHOT_HOTKEYS;
use crate::paint::{Card, H};
use crate::screenshot::store;
use crate::settings_theme::color;
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, RichText};

/// 设置窗口中截图分组的编号。
pub(crate) const SETTINGS_TAB: usize = 14;

impl App {
    /// 截图图库页（第 5 步补全浏览、预览和操作；目前只有入口）。
    pub(crate) fn page_screenshot(&mut self, c: &Card, _env: &Env, cmds: &mut Vec<Cmd>) {
        c.text(209.5, 64.0, "还没有截图", size::BODY, W::Semibold, theme::WHITE, H::Center);
        let hint = if self.screenshot_hotkey_label.is_empty() { "使用托盘「区域截图」".to_string() } else { format!("按 {} 或托盘「区域截图」", self.screenshot_hotkey_label) };
        c.text(209.5, 84.0, &hint, size::SMALL, W::Regular, theme::GRAY, H::Center);
        let button = c.hit(159.5, 96.0, 100.0, 28.0, "screenshot-start");
        let (hover, press) = c.hover_t(&button);
        c.fill(159.5 + press, 96.0 + press, 100.0 - press * 2.0, 28.0 - press * 2.0, 14.0, crate::paint::mix(theme::CARD, theme::BLUE, 0.35 + hover * 0.25));
        c.text(209.5, 115.0, "开始截图", size::SMALL, W::Semibold, theme::WHITE, H::Center);
        if button.clicked() { cmds.push(Cmd::BeginScreenshot); }
    }

    pub(crate) fn screenshot_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let dim = color(&ctx, Color32::from_gray(180), Color32::from_gray(75));
        let warn = color(&ctx, Color32::from_rgb(255, 170, 110), Color32::from_rgb(175, 85, 20));
        ui.label(RichText::new("框选屏幕区域截图：保存为 PNG，同时复制到剪贴板。").size(12.0).color(dim));

        settings_label(ui, "显示");
        settings_card(ui, |ui| {
            let mut visible = self.settings.visible_pages[SCREENSHOT_PAGE];
            setting_row(ui, "截图页", "在岛上浏览截图历史；关闭后快捷键和托盘「区域截图」一并停用", &mut visible);
            if visible != self.settings.visible_pages[SCREENSHOT_PAGE] { self.set_page_visible(SCREENSHOT_PAGE, visible); }
        });

        settings_label(ui, "保存位置");
        settings_card(ui, |ui| {
            let root = store::resolve_root(&self.settings.screenshot_dir);
            let shown = root.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "未设置：没有 E 盘，请选择一个文件夹".into());
            ui.add(egui::Label::new(RichText::new(shown).size(13.0).color(if root.is_some() { ui.visuals().text_color() } else { warn })).truncate());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("选择文件夹…").clicked() { self.pending.push(Cmd::ScreenshotChooseFolder); }
                if let Some(root) = root.as_ref().filter(|path| path.is_dir()) {
                    if ui.button("打开文件夹").clicked() { self.pending.push(Cmd::Open(root.clone())); }
                }
                if !self.settings.screenshot_dir.is_empty() && ui.button("恢复默认").on_hover_text(store::DEFAULT_DIR).clicked() {
                    self.settings.screenshot_dir.clear();
                }
            });
            ui.label(RichText::new("截图按月份放在子文件夹里，只由你手动删除。更换位置后，原来的截图留在原处，切回即可再次看到；不会自动改存到 C 盘。").size(11.0).color(ui.visuals().weak_text_color()));
        });

        settings_label(ui, "快捷键");
        settings_card(ui, |ui| {
            let current = SCREENSHOT_HOTKEYS.iter().find(|(key, _)| *key == self.settings.screenshot_hotkey).map_or("Alt + A", |(_, label)| label);
            settings_control_row(ui, "截图", 64.0, |ui| {
                egui::ComboBox::from_id_salt("screenshot-hotkey").selected_text(current).width(160.0).show_ui(ui, |ui| {
                    for (key, label) in SCREENSHOT_HOTKEYS {
                        ui.selectable_value(&mut self.settings.screenshot_hotkey, key.to_string(), label);
                    }
                }).response
            });
            if !self.settings.visible_pages[SCREENSHOT_PAGE] {
                ui.label(RichText::new("截图页已关闭，快捷键暂不生效。").size(11.5).color(dim));
            } else if !self.screenshot_hotkey_status.is_empty() {
                let ok = !self.screenshot_hotkey_label.is_empty() || self.settings.screenshot_hotkey == "off";
                ui.label(RichText::new(&self.screenshot_hotkey_status).size(11.5).color(if ok { dim } else { warn }));
            }
            ui.label(RichText::new("快捷键对所有程序生效，会覆盖其他程序里的同名快捷键。Alt + A 也是微信的默认截图键，两个程序谁先启动谁生效。").size(11.0).color(ui.visuals().weak_text_color()));
        });
    }

    /// 外壳选好文件夹后调用；保存失败时恢复原值。
    pub fn set_screenshot_dir(&mut self, dir: std::path::PathBuf) {
        let before = self.settings.clone();
        self.settings.screenshot_dir = dir.display().to_string();
        if self.persist_settings(before) { self.say(format!("截图将保存到 {}", self.settings.screenshot_dir)); }
    }

    /// 外壳把托盘线程的注册结果转给设置页；`label` 为空表示当前没有生效的快捷键。
    pub fn set_screenshot_hotkey_status(&mut self, label: &str, message: String) {
        self.screenshot_hotkey_label = label.to_string();
        self.screenshot_hotkey_status = message;
    }
}
