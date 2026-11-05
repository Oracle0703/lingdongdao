//! 设置 → AI 额度与模型 → 大模型：配置内网 OpenAI 兼容服务（如 gpt-oss）并测试连接。
use crate::{
    app::App,
    extras::{setting_row, settings_card, settings_control_row},
    llm,
    settings_theme::color,
};
use eframe::egui::{self, Color32};
use std::sync::mpsc;

enum Outcome {
    Test(Result<String, String>),
    Models(Result<Vec<String>, String>),
}

#[derive(Default)]
pub(crate) struct State {
    /// 表单中的配置；首次打开该设置页时从磁盘加载。
    config: Option<llm::Config>,
    saved: Option<llm::Config>,
    key_input: String,
    show_key: bool,
    has_key: bool,
    models: Vec<String>,
    note: String,
    note_error: bool,
    task: Option<mpsc::Receiver<Outcome>>,
}

impl State {
    fn load(&mut self) {
        if self.config.is_some() {
            return;
        }
        let config = llm::Config::load();
        self.saved = Some(config.clone());
        self.config = Some(config);
        self.has_key = llm::secrets::load().is_some();
    }

    fn poll(&mut self) {
        let Some(rx) = &self.task else { return };
        match rx.try_recv() {
            Ok(Outcome::Test(result)) => {
                self.task = None;
                self.set_note(result);
            }
            Ok(Outcome::Models(Ok(models))) => {
                self.task = None;
                if models.is_empty() {
                    self.set_note(Err("服务没有列出可用模型".into()));
                } else {
                    self.set_note(Ok(format!("找到 {} 个模型，可点击「选择」切换", models.len())));
                }
                self.models = models;
            }
            Ok(Outcome::Models(Err(error))) => {
                self.task = None;
                self.set_note(Err(error));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.task = None;
                self.set_note(Err("请求意外中断，请重试".into()));
            }
        }
    }

    fn set_note(&mut self, result: Result<String, String>) {
        self.note_error = result.is_err();
        self.note = result.unwrap_or_else(|error| error);
    }

    /// 用表单中的配置创建客户端；新输入的 API Key 优先，否则使用已保存的。
    fn client(&self) -> Result<llm::Client, String> {
        let config = self.config.clone().unwrap_or_default();
        let key = if self.key_input.trim().is_empty() { llm::secrets::load() } else { Some(self.key_input.clone()) };
        llm::Client::new(config, key)
    }

    fn spawn(&mut self, ctx: &egui::Context, note: &str, job: impl FnOnce(llm::Client) -> Outcome + Send + 'static) {
        let client = match self.client() {
            Ok(client) => client,
            Err(error) => return self.set_note(Err(error)),
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job(client));
            ctx.request_repaint();
        });
        self.task = Some(rx);
        self.set_note(Ok(note.into()));
    }

    fn save(&mut self) {
        let Some(config) = self.config.clone() else { return };
        if config.enabled {
            if let Err(error) = llm::endpoint(&config.base_url, "chat/completions") {
                return self.set_note(Err(error));
            }
            if config.model.trim().is_empty() {
                return self.set_note(Err("请先填写模型名称".into()));
            }
        }
        if !self.key_input.trim().is_empty() {
            if let Err(error) = llm::secrets::save(&self.key_input) {
                return self.set_note(Err(error));
            }
            self.key_input.clear();
            self.has_key = true;
        }
        match config.save() {
            Ok(()) => {
                self.saved = Some(config);
                self.set_note(Ok("已保存".into()));
            }
            Err(error) => self.set_note(Err(format!("保存失败：{error}"))),
        }
    }
}

impl App {
    pub(crate) fn llm_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let dim = color(&ctx, Color32::from_gray(180), Color32::from_gray(75));
        let state = &mut self.llm;
        state.load();
        state.poll();
        ui.label(egui::RichText::new("接入内网 OpenAI 兼容服务（如 gpt-oss），供后续 AI 功能使用。").size(12.0).color(dim));
        {
            let config = state.config.as_mut().expect("已加载配置");
            let models = &state.models;
            let key_input = &mut state.key_input;
            let show_key = &mut state.show_key;
            let has_key = state.has_key;
            ui.add_space(4.0);
            settings_card(ui, |ui| {
                setting_row(ui, "启用大模型", "关闭时不会向模型服务发送任何内容", &mut config.enabled);
                ui.separator();
                let label = 72.0;
                let gap = ui.spacing().item_spacing.x;
                let width = (ui.available_width() - label - gap).max(140.0);
                settings_control_row(ui, "接口地址", label, |ui| {
                    ui.add_sized(
                        [width, 30.0],
                        egui::TextEdit::singleline(&mut config.base_url)
                            .vertical_align(egui::Align::Center)
                            .hint_text("http://10.0.0.5:8000/v1"),
                    )
                });
                settings_control_row(ui, "模型", label, |ui| {
                    let field = ui.add_sized(
                        [(width - 56.0 - gap).max(80.0), 30.0],
                        egui::TextEdit::singleline(&mut config.model)
                            .vertical_align(egui::Align::Center)
                            .hint_text("gpt-oss-20b"),
                    );
                    ui.add_enabled_ui(!models.is_empty(), |ui| {
                        ui.menu_button("选择", |ui| {
                            for name in models {
                                if ui.selectable_label(config.model == *name, name).clicked() {
                                    config.model = name.clone();
                                    ui.close_menu();
                                }
                            }
                        })
                    })
                    .response
                    .on_disabled_hover_text("先点击「获取模型列表」");
                    field
                });
                settings_control_row(ui, "API Key", label, |ui| {
                    let field = ui.add_sized(
                        [(width - 48.0 - gap).max(80.0), 30.0],
                        egui::TextEdit::singleline(key_input)
                            .vertical_align(egui::Align::Center)
                            .password(!*show_key)
                            .hint_text(if has_key { "已保存；留空则保持不变" } else { "服务不需要密钥时可留空" }),
                    );
                    if ui.add_sized([48.0, 30.0], egui::Button::new(if *show_key { "隐藏" } else { "显示" })).clicked() {
                        *show_key = !*show_key;
                    }
                    field
                });
                settings_control_row(ui, "推理强度", label, |ui| {
                    let effort = egui::ComboBox::from_id_salt("llm-effort")
                        .selected_text(config.effort.label())
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for effort in llm::Effort::ALL {
                                ui.selectable_value(&mut config.effort, effort, effort.label());
                            }
                        })
                        .response;
                    ui.add_space(8.0);
                    ui.label("超时");
                    ui.add(egui::DragValue::new(&mut config.timeout_secs).range(llm::Config::TIMEOUT_RANGE).suffix(" 秒"));
                    effort
                });
            });
        }

        let busy = state.task.is_some();
        let (mut test, mut list, mut save, mut clear) = (false, false, false, false);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            test = ui.add_enabled(!busy, egui::Button::new("测试连接")).clicked();
            list = ui.add_enabled(!busy, egui::Button::new("获取模型列表")).clicked();
            save = ui.button("保存").clicked();
            clear = state.has_key && ui.button("清除 API Key").clicked();
            if state.config != state.saved {
                ui.label(egui::RichText::new("有未保存的修改").size(12.0).color(dim));
            }
        });
        if test {
            state.spawn(&ctx, "正在测试连接…", |client| Outcome::Test(client.test()));
        }
        if list {
            state.spawn(&ctx, "正在获取模型列表…", |client| Outcome::Models(client.models()));
        }
        if save {
            state.save();
        }
        if clear {
            llm::secrets::delete();
            state.has_key = false;
            state.set_note(Ok("已清除 API Key".into()));
        }
        if !state.note.is_empty() {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if state.task.is_some() {
                    ui.spinner();
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                let tint = if state.note_error { color(&ctx, Color32::from_rgb(255, 130, 120), Color32::from_rgb(190, 45, 35)) } else { dim };
                ui.add(egui::Label::new(egui::RichText::new(&state.note).size(12.0).color(tint)).wrap());
            });
        }
        ui.add_space(6.0);
        for line in [
            "API Key 存储在 Windows 凭据管理器中。",
            "HTTPS 使用 Windows 证书库校验，内网 CA 证书导入系统后即可使用。",
            "gpt-oss 建议推理强度选「低」，响应更快；推理内容不会显示。",
        ] {
            ui.label(egui::RichText::new(line).size(12.0).color(dim));
        }
    }
}
