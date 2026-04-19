use std::collections::HashMap;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::bedrock::{self, StreamToken};
use crate::db::Database;
use crate::message::{ChatMessage, Conversation, Role, MODELS, REGIONS};

// ── Colors ─────────────────────────────────────────────────────────────

mod colors {
    use eframe::egui::Color32;

    pub const BG_BASE: Color32 = Color32::from_rgb(22, 22, 26);
    pub const BG_SIDEBAR: Color32 = Color32::from_rgb(28, 28, 33);
    pub const BG_USER_MSG: Color32 = Color32::from_rgb(32, 33, 42);
    pub const BG_ASSISTANT_MSG: Color32 = Color32::from_rgb(26, 26, 30);
    pub const BG_INPUT: Color32 = Color32::from_rgb(34, 35, 40);
    pub const BG_TOPBAR: Color32 = Color32::from_rgb(28, 28, 33);
    pub const BG_MODAL_CARD: Color32 = Color32::from_rgb(32, 33, 38);

    pub const ACCENT: Color32 = Color32::from_rgb(100, 140, 255);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(70, 100, 190);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(220, 222, 228);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(140, 144, 158);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(90, 94, 108);
    pub const ERROR: Color32 = Color32::from_rgb(255, 110, 110);
    pub const BORDER: Color32 = Color32::from_rgb(50, 52, 60);
    pub const HOVER: Color32 = Color32::from_rgb(42, 44, 54);
    pub const SELECTED: Color32 = Color32::from_rgb(40, 50, 75);

    pub const ROLE_USER: Color32 = Color32::from_rgb(130, 170, 255);
    pub const ROLE_ASSISTANT: Color32 = Color32::from_rgb(160, 220, 160);
}

// ── Credential modal state ─────────────────────────────────────────────

#[derive(Default)]
struct CredentialForm {
    api_key: String,
    region_idx: usize,
}

// ── App state ──────────────────────────────────────────────────────────

enum Screen {
    Credentials(CredentialForm),
    Chat,
}

pub struct ChatApp {
    rt: tokio::runtime::Handle,
    db: Database,
    screen: Screen,

    conversations: Vec<Conversation>,
    active_id: Option<String>,
    messages: Vec<ChatMessage>,
    md_caches: HashMap<String, (u64, CommonMarkCache)>,
    streaming_md_cache: CommonMarkCache,
    input: String,
    stream_rx: Option<mpsc::UnboundedReceiver<StreamToken>>,
    is_streaming: bool,
    last_error: Option<String>,
    scroll_to_bottom: bool,
    model_idx: usize,
    region_idx: usize,
    show_system_prompt: bool,
    clipboard: Option<arboard::Clipboard>,
}

impl ChatApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        configure_visuals(&cc.egui_ctx);

        let db = match Database::open() {
            Ok(db) => db,
            Err(e) => {
                error!("Failed to open database: {e:#}");
                panic!("Cannot open database: {e:#}");
            }
        };

        let conversations = db.list_conversations().unwrap_or_default();

        // Try to restore saved API key — skip the modal if we have one
        let saved_key = db.get_config("api_key").ok().flatten();
        let saved_region = db
            .get_config("region")
            .ok()
            .flatten()
            .and_then(|r| REGIONS.iter().position(|&x| x == r))
            .unwrap_or(0);

        let screen = if let Some(ref key) = saved_key {
            if !key.is_empty() {
                std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", key);
                Screen::Chat
            } else {
                Screen::Credentials(CredentialForm::default())
            }
        } else {
            Screen::Credentials(CredentialForm::default())
        };

        let clipboard = arboard::Clipboard::new()
            .map_err(|e| warn!("clipboard unavailable: {e}"))
            .ok();

        Self {
            rt,
            db,
            screen,
            conversations,
            active_id: None,
            messages: Vec::new(),
            md_caches: HashMap::new(),
            streaming_md_cache: CommonMarkCache::default(),
            input: String::new(),
            stream_rx: None,
            is_streaming: false,
            last_error: None,
            scroll_to_bottom: false,
            model_idx: 0,
            region_idx: saved_region,
            show_system_prompt: false,
            clipboard,
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn active_conversation(&self) -> Option<&Conversation> {
        self.active_id
            .as_ref()
            .and_then(|id| self.conversations.iter().find(|c| c.id == *id))
    }

    fn active_conversation_mut(&mut self) -> Option<&mut Conversation> {
        let id = self.active_id.clone()?;
        self.conversations.iter_mut().find(|c| c.id == id)
    }

    fn select_conversation(&mut self, id: &str) {
        self.active_id = Some(id.to_string());
        match self.db.list_messages(id) {
            Ok(msgs) => {
                self.messages = msgs;
                self.md_caches.clear();
                self.scroll_to_bottom = true;
            }
            Err(e) => {
                error!("failed to load messages: {e:#}");
                self.last_error = Some(format!("Failed to load messages: {e:#}"));
            }
        }
        let conv_data = self
            .active_conversation()
            .map(|c| (c.model_id.clone(), c.region.clone()));
        if let Some((model_id, region)) = conv_data {
            if let Some(idx) = MODELS.iter().position(|(_, mid)| *mid == model_id) {
                self.model_idx = idx;
            }
            if let Some(idx) = REGIONS.iter().position(|r| *r == region) {
                self.region_idx = idx;
            }
        }
    }

    fn new_conversation(&mut self) {
        let model_id = MODELS[self.model_idx].1;
        let region = REGIONS[self.region_idx];
        let conv = Conversation::new("New Chat", model_id, region);
        if let Err(e) = self.db.upsert_conversation(&conv) {
            error!("failed to create conversation: {e:#}");
            self.last_error = Some(format!("DB error: {e:#}"));
            return;
        }
        let id = conv.id.clone();
        self.conversations.insert(0, conv);
        self.select_conversation(&id);
    }

    fn delete_conversation(&mut self, id: &str) {
        if let Err(e) = self.db.delete_conversation(id) {
            error!("failed to delete conversation: {e:#}");
            self.last_error = Some(format!("DB error: {e:#}"));
            return;
        }
        self.conversations.retain(|c| c.id != id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
            self.messages.clear();
            self.md_caches.clear();
        }
    }

    fn send_message(&mut self, ctx: &egui::Context) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let conv_id = match &self.active_id {
            Some(id) => id.clone(),
            None => {
                self.new_conversation();
                match &self.active_id {
                    Some(id) => id.clone(),
                    None => return,
                }
            }
        };

        let user_msg = ChatMessage::new(&conv_id, Role::User, &text);
        if let Err(e) = self.db.insert_message(&user_msg) {
            error!("failed to insert user message: {e:#}");
            self.last_error = Some(format!("DB error: {e:#}"));
            return;
        }
        self.messages.push(user_msg);
        self.input.clear();

        if self.messages.len() == 1 {
            let title: String = text.chars().take(50).collect();
            if let Some(conv) = self.active_conversation_mut() {
                conv.title = title;
                conv.updated_at = chrono::Utc::now();
            }
            if let Some(conv) = self.active_conversation() {
                let _ = self.db.upsert_conversation(conv);
            }
        }

        let assistant_msg = ChatMessage::new(&conv_id, Role::Assistant, "");
        if let Err(e) = self.db.insert_message(&assistant_msg) {
            error!("failed to insert assistant message: {e:#}");
        }
        self.messages.push(assistant_msg);

        let history: Vec<(String, String)> = self
            .messages
            .iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| (m.role.as_str().to_string(), m.content.clone()))
            .collect();

        let conv_info = self
            .active_conversation()
            .map(|c| (c.model_id.clone(), c.region.clone(), c.system_prompt.clone()));
        let (model_id, region, system_prompt) = match conv_info {
            Some(t) => t,
            None => return,
        };

        self.streaming_md_cache = CommonMarkCache::default();
        let rx = bedrock::spawn_stream(
            &self.rt,
            ctx.clone(),
            model_id,
            region,
            system_prompt,
            history,
        );
        self.stream_rx = Some(rx);
        self.is_streaming = true;
        self.scroll_to_bottom = true;
    }

    fn poll_stream(&mut self) {
        let rx = match &mut self.stream_rx {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match rx.try_recv() {
                Ok(StreamToken::Delta(text)) => {
                    if let Some(msg) = self.messages.last_mut() {
                        msg.append_token(&text);
                        self.scroll_to_bottom = true;
                    }
                }
                Ok(StreamToken::Done) => {
                    info!("stream completed");
                    self.finish_stream();
                    break;
                }
                Ok(StreamToken::Error(e)) => {
                    self.last_error = Some(e);
                    self.finish_stream();
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("stream channel disconnected");
                    self.finish_stream();
                    break;
                }
            }
        }
    }

    fn finish_stream(&mut self) {
        self.is_streaming = false;
        self.stream_rx = None;

        if let Some(msg) = self.messages.last() {
            if msg.role == Role::Assistant {
                if let Err(e) = self.db.update_message_content(&msg.id, &msg.content) {
                    error!("failed to update message content: {e:#}");
                }
            }
        }

        if let Some(conv) = self.active_conversation_mut() {
            conv.updated_at = chrono::Utc::now();
        }
        if let Some(conv) = self.active_conversation() {
            let _ = self.db.upsert_conversation(conv);
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        if let Some(ref mut cb) = self.clipboard {
            if let Err(e) = cb.set_text(text) {
                warn!("clipboard copy failed: {e}");
            }
        }
    }

    // ── Credential modal ───────────────────────────────────────────────

    fn render_credentials_modal(&mut self, ui: &mut egui::Ui) {
        // Full-screen dark background
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, colors::BG_BASE);

        ui.vertical_centered(|ui| {
            ui.add_space(rect.height() * 0.28);

            egui::Frame::new()
                .inner_margin(egui::Margin::same(32))
                .corner_radius(16.0)
                .fill(colors::BG_MODAL_CARD)
                .stroke(egui::Stroke::new(1.0, colors::BORDER))
                .show(ui, |ui| {
                    ui.set_width(400.0);

                    ui.colored_label(colors::TEXT_PRIMARY, egui::RichText::new("Bedrock Chat").size(24.0).strong());
                    ui.add_space(6.0);
                    ui.colored_label(
                        colors::TEXT_SECONDARY,
                        "Paste your Bedrock API key, or skip to use\nyour existing AWS config.",
                    );
                    ui.add_space(16.0);

                    let Screen::Credentials(form) = &mut self.screen else {
                        return;
                    };

                    ui.colored_label(colors::TEXT_SECONDARY, "API Key");
                    ui.add_space(2.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut form.api_key)
                            .desired_width(f32::INFINITY)
                            .password(true)
                            .hint_text("Paste Bedrock API key..."),
                    );
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.colored_label(colors::TEXT_SECONDARY, "Region");
                        ui.add_space(4.0);
                        egui::ComboBox::from_id_salt("cred_region")
                            .selected_text(REGIONS[form.region_idx])
                            .show_ui(ui, |ui| {
                                for (i, region) in REGIONS.iter().enumerate() {
                                    ui.selectable_value(&mut form.region_idx, i, *region);
                                }
                            });
                    });

                    ui.add_space(20.0);

                    ui.horizontal(|ui| {
                        let Screen::Credentials(form) = &self.screen else {
                            return;
                        };
                        let has_key = !form.api_key.trim().is_empty();

                        if ui
                            .add_enabled(has_key, egui::Button::new(
                                egui::RichText::new("Connect").color(if has_key { colors::BG_BASE } else { colors::TEXT_MUTED })
                            ).fill(if has_key { colors::ACCENT } else { colors::BG_INPUT }).corner_radius(8.0).min_size(egui::vec2(90.0, 32.0)))
                            .clicked()
                        {
                            let Screen::Credentials(form) = &self.screen else {
                                return;
                            };
                            let key = form.api_key.trim().to_string();
                            std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", &key);
                            let _ = self.db.set_config("api_key", &key);
                            let _ = self.db.set_config("region", REGIONS[form.region_idx]);
                            self.region_idx = form.region_idx;
                            self.screen = Screen::Chat;
                        }

                        ui.add_space(8.0);

                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("Skip").color(colors::TEXT_SECONDARY)
                            ).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, colors::BORDER)).corner_radius(8.0).min_size(egui::vec2(70.0, 32.0)))
                            .clicked()
                        {
                            let Screen::Credentials(form) = &self.screen else {
                                return;
                            };
                            let _ = self.db.set_config("region", REGIONS[form.region_idx]);
                            self.region_idx = form.region_idx;
                            self.screen = Screen::Chat;
                        }
                    });
                });
        });
    }

    // ── Sidebar ────────────────────────────────────────────────────────

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, colors::BG_SIDEBAR);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.colored_label(colors::TEXT_PRIMARY, egui::RichText::new("Chats").size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("+").size(16.0).color(colors::ACCENT))
                            .fill(egui::Color32::TRANSPARENT)
                            .corner_radius(6.0)
                            .min_size(egui::vec2(28.0, 28.0)),
                    )
                    .clicked()
                {
                    self.new_conversation();
                }
            });
        });
        ui.add_space(4.0);

        // Thin separator
        let rect = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [rect.left_top(), egui::pos2(rect.right(), rect.top())],
            egui::Stroke::new(1.0, colors::BORDER),
        );
        ui.add_space(6.0);

        let active_id = self.active_id.clone();
        let mut to_select: Option<String> = None;
        let mut to_delete: Option<String> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for conv in &self.conversations {
                let is_active = active_id.as_deref() == Some(&conv.id);
                let bg = if is_active {
                    colors::SELECTED
                } else {
                    egui::Color32::TRANSPARENT
                };

                let title: String = if conv.title.len() > 28 {
                    format!("{}...", &conv.title[..25])
                } else {
                    conv.title.clone()
                };

                egui::Frame::new()
                    .fill(bg)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .outer_margin(egui::Margin::symmetric(4, 1))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let text_color = if is_active {
                                colors::TEXT_PRIMARY
                            } else {
                                colors::TEXT_SECONDARY
                            };
                            let resp = ui.add(
                                egui::Label::new(egui::RichText::new(&title).color(text_color).size(13.0))
                                    .selectable(false)
                                    .sense(egui::Sense::click()),
                            );
                            if resp.clicked() && !is_active {
                                to_select = Some(conv.id.clone());
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if is_active || ui.rect_contains_pointer(ui.max_rect()) {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("x").color(colors::TEXT_MUTED).size(12.0),
                                            )
                                            .fill(egui::Color32::TRANSPARENT)
                                            .min_size(egui::vec2(20.0, 20.0)),
                                        )
                                        .clicked()
                                    {
                                        to_delete = Some(conv.id.clone());
                                    }
                                }
                            });
                        });
                    });
            }
        });

        if let Some(id) = to_select {
            self.select_conversation(&id);
        }
        if let Some(id) = to_delete {
            self.delete_conversation(&id);
        }
    }

    // ── Chat pane ──────────────────────────────────────────────────────

    fn render_chat_pane(&mut self, ui: &mut egui::Ui) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, colors::BG_BASE);

        if self.active_id.is_none() {
            ui.centered_and_justified(|ui| {
                ui.colored_label(colors::TEXT_MUTED, "Select or create a conversation");
            });
            return;
        }

        self.render_top_bar(ui);

        // Thin separator
        let rect = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [rect.left_top(), egui::pos2(rect.right(), rect.top())],
            egui::Stroke::new(1.0, colors::BORDER),
        );
        ui.add_space(2.0);

        // Messages take remaining space minus input
        let input_area_height = 100.0;
        let avail = ui.available_height() - input_area_height;
        ui.allocate_ui(egui::vec2(ui.available_width(), avail.max(100.0)), |ui| {
            self.render_messages(ui);
        });

        // Error display
        if let Some(err) = self.last_error.clone() {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.colored_label(colors::ERROR, &err);
                if ui.small_button("dismiss").clicked() {
                    self.last_error = None;
                }
            });
        }

        self.render_input(ui);
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(colors::BG_TOPBAR)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(colors::TEXT_SECONDARY, "Model");
                    ui.add_space(2.0);
                    let current_model_name = MODELS[self.model_idx].0;
                    egui::ComboBox::from_id_salt("model_picker")
                        .selected_text(current_model_name)
                        .show_ui(ui, |ui| {
                            for (i, (name, _)) in MODELS.iter().enumerate() {
                                ui.selectable_value(&mut self.model_idx, i, *name);
                            }
                        });

                    ui.add_space(12.0);
                    ui.colored_label(colors::TEXT_SECONDARY, "Region");
                    ui.add_space(2.0);
                    let current_region = REGIONS[self.region_idx];
                    egui::ComboBox::from_id_salt("region_picker")
                        .selected_text(current_region)
                        .show_ui(ui, |ui| {
                            for (i, region) in REGIONS.iter().enumerate() {
                                ui.selectable_value(&mut self.region_idx, i, *region);
                            }
                        });

                    ui.add_space(12.0);
                    if ui
                        .selectable_label(self.show_system_prompt, "System Prompt")
                        .clicked()
                    {
                        self.show_system_prompt = !self.show_system_prompt;
                    }
                });
            });

        // Sync model/region
        let model_id = MODELS[self.model_idx].1.to_string();
        let region = REGIONS[self.region_idx].to_string();
        if let Some(conv) = self.active_conversation_mut() {
            if conv.model_id != model_id || conv.region != region {
                conv.model_id = model_id;
                conv.region = region;
            }
        }
        if let Some(conv) = self.active_conversation() {
            let _ = self.db.upsert_conversation(conv);
        }

        if self.show_system_prompt {
            egui::Frame::new()
                .fill(colors::BG_TOPBAR)
                .inner_margin(egui::Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    let mut sys = self
                        .active_conversation()
                        .map(|c| c.system_prompt.clone())
                        .unwrap_or_default();
                    let changed = ui
                        .add(
                            egui::TextEdit::multiline(&mut sys)
                                .hint_text("Enter system prompt...")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        )
                        .changed();
                    if changed {
                        if let Some(conv) = self.active_conversation_mut() {
                            conv.system_prompt = sys;
                        }
                        if let Some(conv) = self.active_conversation() {
                            let _ = self.db.upsert_conversation(conv);
                        }
                    }
                });
        }
    }

    fn render_messages(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Horizontal padding for message content
                let side_pad = (ui.available_width() * 0.04).clamp(12.0, 40.0);

                ui.add_space(8.0);
                let msg_count = self.messages.len();
                for i in 0..msg_count {
                    let is_last = i == msg_count - 1;
                    let is_streaming_msg = is_last && self.is_streaming;

                    ui.horizontal(|ui| {
                        ui.add_space(side_pad);
                        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                            ui.set_width(ui.available_width() - side_pad);
                            self.render_single_message(ui, i, is_streaming_msg);
                        });
                    });
                }

                if self.scroll_to_bottom {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    self.scroll_to_bottom = false;
                }
            });
    }

    fn render_single_message(&mut self, ui: &mut egui::Ui, idx: usize, is_streaming: bool) {
        let role = self.messages[idx].role;
        let (role_label, role_color, bg_color) = match role {
            Role::User => ("You", colors::ROLE_USER, colors::BG_USER_MSG),
            Role::Assistant => ("Assistant", colors::ROLE_ASSISTANT, colors::BG_ASSISTANT_MSG),
        };

        let content_empty = self.messages[idx].content.is_empty();
        let content_for_copy = if role == Role::Assistant && !content_empty {
            Some(self.messages[idx].content.clone())
        } else {
            None
        };

        egui::Frame::new()
            .fill(bg_color)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .outer_margin(egui::Margin::symmetric(0, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Header row: role label + copy/spinner
                ui.horizontal(|ui| {
                    ui.colored_label(role_color, egui::RichText::new(role_label).size(12.5).strong());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ref text) = content_for_copy {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Copy").size(11.0).color(colors::TEXT_MUTED),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .corner_radius(4.0),
                                )
                                .clicked()
                            {
                                self.copy_to_clipboard(text);
                            }
                        }
                        if is_streaming {
                            ui.spinner();
                        }
                    });
                });

                ui.add_space(6.0);

                if content_empty && is_streaming {
                    ui.colored_label(colors::TEXT_MUTED, "...");
                } else if !content_empty {
                    let content = self.messages[idx].content.clone();
                    if is_streaming {
                        CommonMarkViewer::new()
                            .show(ui, &mut self.streaming_md_cache, &content);
                    } else {
                        let msg_id = self.messages[idx].id.clone();
                        let version = self.messages[idx].version;
                        let entry = self
                            .md_caches
                            .entry(msg_id)
                            .or_insert_with(|| (version, CommonMarkCache::default()));
                        if entry.0 != version {
                            *entry = (version, CommonMarkCache::default());
                        }
                        CommonMarkViewer::new().show(ui, &mut entry.1, &content);
                    }
                }
            });
    }

    fn render_input(&mut self, ui: &mut egui::Ui) {
        let send_shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
        let send_shortcut_ctrl =
            egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Enter);

        egui::Frame::new()
            .fill(colors::BG_BASE)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(colors::BG_INPUT)
                    .corner_radius(12.0)
                    .stroke(egui::Stroke::new(1.0, colors::BORDER))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            let desired_rows = {
                                let line_count =
                                    self.input.chars().filter(|c| *c == '\n').count() + 1;
                                line_count.clamp(1, 8)
                            };

                            let response = ui.add_sized(
                                egui::vec2(ui.available_width() - 60.0, 0.0),
                                egui::TextEdit::multiline(&mut self.input)
                                    .hint_text(egui::RichText::new("Message...").color(colors::TEXT_MUTED))
                                    .desired_rows(desired_rows)
                                    .lock_focus(true)
                                    .text_color(colors::TEXT_PRIMARY),
                            );

                            let ctrl_enter_pressed = ui.input_mut(|i| {
                                i.consume_shortcut(&send_shortcut)
                                    || i.consume_shortcut(&send_shortcut_ctrl)
                            });

                            let can_send = !self.is_streaming && !self.input.trim().is_empty();

                            let btn_color = if can_send { colors::ACCENT } else { colors::ACCENT_DIM };
                            let send_clicked = ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Send")
                                            .color(if can_send { colors::BG_BASE } else { colors::TEXT_MUTED })
                                            .size(13.0),
                                    )
                                    .fill(btn_color)
                                    .corner_radius(8.0)
                                    .min_size(egui::vec2(52.0, 30.0)),
                                )
                                .clicked();

                            if (ctrl_enter_pressed || send_clicked) && can_send {
                                let ctx = ui.ctx().clone();
                                self.send_message(&ctx);
                            }

                            if !self.is_streaming {
                                response.request_focus();
                            }
                        });
                    });
            });
    }
}

// ── Theme configuration ─────────────────────────────────────────────────

fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = colors::BG_BASE;
    visuals.window_fill = colors::BG_BASE;
    visuals.extreme_bg_color = colors::BG_INPUT;
    visuals.faint_bg_color = colors::BG_SIDEBAR;

    visuals.widgets.noninteractive.bg_fill = colors::BG_INPUT;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);

    visuals.widgets.inactive.bg_fill = colors::BG_INPUT;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_SECONDARY);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);

    visuals.widgets.hovered.bg_fill = colors::HOVER;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);

    visuals.widgets.active.bg_fill = colors::SELECTED;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);

    visuals.selection.bg_fill = colors::ACCENT_DIM;
    visuals.selection.stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);

    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors::BORDER);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, colors::BORDER);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_global_style(style);
}

// ── eframe::App ─────────────────────────────────────────────────────────

impl eframe::App for ChatApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &self.screen {
            Screen::Credentials(_) => {
                self.render_credentials_modal(ui);
            }
            Screen::Chat => {
                self.poll_stream();

                egui::Panel::left("sidebar")
                    .default_size(240.0)
                    .min_size(180.0)
                    .show_inside(ui, |ui| {
                        self.render_sidebar(ui);
                    });

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    self.render_chat_pane(ui);
                });
            }
        }
    }
}
