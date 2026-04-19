use std::collections::HashMap;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use rand::Rng;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::bedrock::{self, StreamToken};
use crate::db::Database;
use crate::message::{ChatMessage, Conversation, Role, TokenUsage, MODELS, REGIONS};

// ── Theme-aware color palette ──────────────────────────────────────────

#[derive(Clone)]
struct Palette {
    bg_base: egui::Color32,
    bg_sidebar: egui::Color32,
    bg_user_msg: egui::Color32,
    bg_assist_msg: egui::Color32,
    bg_input: egui::Color32,
    bg_topbar: egui::Color32,
    bg_modal: egui::Color32,
    accent: egui::Color32,
    accent_dim: egui::Color32,
    accent2: egui::Color32,  // secondary accent for particles
    accent3: egui::Color32,  // tertiary accent for particles
    text_primary: egui::Color32,
    text_secondary: egui::Color32,
    text_muted: egui::Color32,
    error: egui::Color32,
    border: egui::Color32,
    hover: egui::Color32,
    selected: egui::Color32,
    role_user: egui::Color32,
    role_assistant: egui::Color32,
}

impl Palette {
    fn dark() -> Self {
        Self {
            bg_base: c(22, 22, 26), bg_sidebar: c(28, 28, 33), bg_user_msg: c(32, 33, 42),
            bg_assist_msg: c(26, 26, 30), bg_input: c(34, 35, 40), bg_topbar: c(28, 28, 33),
            bg_modal: c(32, 33, 38),
            accent: c(100, 140, 255), accent_dim: c(70, 100, 190),
            accent2: c(160, 100, 255), accent3: c(80, 200, 200),
            text_primary: c(220, 222, 228), text_secondary: c(140, 144, 158),
            text_muted: c(90, 94, 108), error: c(255, 110, 110), border: c(50, 52, 60),
            hover: c(42, 44, 54), selected: c(40, 50, 75),
            role_user: c(130, 170, 255), role_assistant: c(160, 220, 160),
        }
    }
    fn light() -> Self {
        Self {
            bg_base: c(245, 245, 248), bg_sidebar: c(235, 236, 240), bg_user_msg: c(225, 230, 245),
            bg_assist_msg: c(240, 240, 244), bg_input: c(255, 255, 255), bg_topbar: c(235, 236, 240),
            bg_modal: c(255, 255, 255),
            accent: c(50, 100, 220), accent_dim: c(130, 160, 220),
            accent2: c(120, 60, 200), accent3: c(40, 160, 160),
            text_primary: c(30, 30, 36), text_secondary: c(90, 94, 108),
            text_muted: c(150, 154, 168), error: c(200, 50, 50), border: c(210, 212, 220),
            hover: c(220, 222, 230), selected: c(210, 220, 245),
            role_user: c(40, 80, 180), role_assistant: c(30, 130, 50),
        }
    }
    fn for_theme(theme: egui::Theme) -> Self {
        match theme { egui::Theme::Dark => Self::dark(), egui::Theme::Light => Self::light() }
    }
}
fn c(r: u8, g: u8, b: u8) -> egui::Color32 { egui::Color32::from_rgb(r, g, b) }

// ── Burst particle effect ───────────────────────────────────────────────
// Short-lived particle burst spawned from a point (e.g. the + button on new chat).

struct BurstParticle {
    x: f32, y: f32,
    vx: f32, vy: f32,
    radius: f32,
    color_idx: u8,
    life: f32,     // 0..1, counts down
}

struct BurstFx {
    particles: Vec<BurstParticle>,
}

impl BurstFx {
    fn new() -> Self { Self { particles: Vec::new() } }

    /// Spawn a burst of particles from a screen position.
    fn spawn(&mut self, pos: egui::Pos2, count: usize) {
        let mut rng = rand::thread_rng();
        for _ in 0..count {
            let angle = rng.gen_range(0.0..std::f32::consts::TAU);
            let speed = rng.gen_range(80.0..250.0);
            self.particles.push(BurstParticle {
                x: pos.x, y: pos.y,
                vx: angle.cos() * speed,
                vy: angle.sin() * speed,
                radius: rng.gen_range(1.5..4.0),
                color_idx: rng.gen_range(0..3),
                life: 1.0,
            });
        }
    }

    /// Returns true if there are active particles (caller should request_repaint).
    fn draw(&mut self, painter: &egui::Painter, pal: &Palette, dt: f32) -> bool {
        let decay = 2.5; // life units per second
        self.particles.retain_mut(|p| {
            p.life -= decay * dt;
            if p.life <= 0.0 { return false; }

            // Decelerate
            p.vx *= 1.0 - 3.0 * dt;
            p.vy *= 1.0 - 3.0 * dt;
            // Gravity
            p.vy += 60.0 * dt;

            p.x += p.vx * dt;
            p.y += p.vy * dt;

            let a = (p.life * 200.0) as u8;
            let base = match p.color_idx {
                0 => pal.accent, 1 => pal.accent2, _ => pal.accent3,
            };
            let color = egui::Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), a);
            let r = p.radius * (0.5 + p.life * 0.5);
            painter.circle_filled(egui::pos2(p.x, p.y), r, color);
            true
        });
        !self.particles.is_empty()
    }
}

// ── App state ──────────────────────────────────────────────────────────

#[derive(Default)]
struct CredentialForm { api_key: String, region_idx: usize, is_settings: bool }

enum Screen { Credentials(CredentialForm), Chat }

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
    /// True when the user has scrolled up during streaming — suppresses auto-scroll
    user_scrolled_up: bool,
    model_idx: usize,
    region_idx: usize,
    show_system_prompt: bool,
    system_prompt_draft: String,
    clipboard: Option<arboard::Clipboard>,

    conv_usage: TokenUsage,
    last_usage: Option<TokenUsage>,

    current_theme: egui::Theme,
    pal: Palette,
    burst: BurstFx,
    last_frame_time: Option<f64>,

    model_filter: String,
    show_model_picker: bool,
    model_picker_hover_idx: Option<usize>,
    model_picker_btn_rect: Option<egui::Rect>,
    ephemeral: bool,

    show_search: bool,
    search_query: String,
    search_results: Vec<(String, String, String)>,
    /// Whether search modal just opened (for one-shot focus)
    search_just_opened: bool,
    search_selected_idx: usize,

    is_compacting: bool,
    compact_rx: Option<mpsc::UnboundedReceiver<StreamToken>>,

    /// Timestamp of last Escape press for double-tap detection
    last_escape_time: f64,
}

impl ChatApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        let theme = cc.egui_ctx.system_theme().unwrap_or(egui::Theme::Dark);
        let pal = Palette::for_theme(theme);
        apply_visuals(&cc.egui_ctx, theme, &pal);

        let db = match Database::open() {
            Ok(db) => db,
            Err(e) => { error!("Failed to open database: {e:#}"); panic!("Cannot open database: {e:#}"); }
        };

        let conversations = db.list_conversations().unwrap_or_default();
        let saved_key = db.get_config("api_key").ok().flatten();
        let saved_region = db.get_config("region").ok().flatten()
            .and_then(|r| REGIONS.iter().position(|&x| x == r)).unwrap_or(0);

        let screen = if let Some(ref key) = saved_key {
            if !key.is_empty() { std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", key); Screen::Chat }
            else { Screen::Credentials(CredentialForm::default()) }
        } else { Screen::Credentials(CredentialForm::default()) };

        let clipboard = arboard::Clipboard::new().map_err(|e| warn!("clipboard unavailable: {e}")).ok();

        Self {
            rt, db, screen, conversations, active_id: None, messages: Vec::new(),
            md_caches: HashMap::new(), streaming_md_cache: CommonMarkCache::default(),
            input: String::new(), stream_rx: None, is_streaming: false, last_error: None,
            scroll_to_bottom: false, user_scrolled_up: false, model_idx: 0, region_idx: saved_region,
            show_system_prompt: false, system_prompt_draft: String::new(), clipboard, conv_usage: TokenUsage::default(),
            last_usage: None, current_theme: theme, pal, burst: BurstFx::new(),
            last_frame_time: None, model_filter: String::new(), show_model_picker: false,
            model_picker_hover_idx: None, model_picker_btn_rect: None, ephemeral: false,
            show_search: false, search_query: String::new(), search_results: Vec::new(),
            search_just_opened: false, search_selected_idx: 0,
            is_compacting: false, compact_rx: None,
            last_escape_time: 0.0,
        }
    }

    fn check_theme(&mut self, ctx: &egui::Context) {
        let theme = ctx.system_theme().unwrap_or(egui::Theme::Dark);
        if theme != self.current_theme {
            self.current_theme = theme;
            self.pal = Palette::for_theme(theme);
            apply_visuals(ctx, theme, &self.pal);
        }
    }

    fn get_dt(&mut self, ui: &egui::Ui) -> f32 {
        let now = ui.input(|i| i.time);
        let dt = self.last_frame_time.map_or(0.016, |t| (now - t) as f32).min(0.1);
        self.last_frame_time = Some(now);
        dt
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn active_conversation(&self) -> Option<&Conversation> {
        self.active_id.as_ref().and_then(|id| self.conversations.iter().find(|c| c.id == *id))
    }
    fn active_conversation_mut(&mut self) -> Option<&mut Conversation> {
        let id = self.active_id.clone()?;
        self.conversations.iter_mut().find(|c| c.id == id)
    }
    fn conv_has_messages(&self) -> bool { !self.messages.is_empty() }

    fn select_conversation(&mut self, id: &str) {
        // Clean up empty ephemeral chat when leaving
        if self.ephemeral {
            if let Some(old_id) = &self.active_id {
                if self.messages.is_empty() && old_id != id {
                    // Remove the empty ephemeral chat from the list
                    let old_id_clone = old_id.clone();
                    self.conversations.retain(|c| c.id != old_id_clone);
                }
            }
        }
        
        self.active_id = Some(id.to_string());
        self.conv_usage = TokenUsage::default();
        self.last_usage = None;
        match self.db.list_messages(id) {
            Ok(msgs) => { self.messages = msgs; self.md_caches.clear(); self.scroll_to_bottom = true; }
            Err(e) => { error!("failed to load messages: {e:#}"); self.last_error = Some(format!("Failed to load: {e:#}")); }
        }
        let conv_data = self.active_conversation().map(|c| (c.model_id.clone(), c.region.clone(), c.system_prompt.clone()));
        if let Some((model_id, region, sys_prompt)) = conv_data {
            if let Some(idx) = MODELS.iter().position(|m| m.id == model_id) { self.model_idx = idx; }
            if let Some(idx) = REGIONS.iter().position(|r| *r == region) { self.region_idx = idx; }
            self.system_prompt_draft = sys_prompt;
        }
    }

    fn new_conversation(&mut self) {
        let model_id = MODELS[self.model_idx].id;
        let region = REGIONS[self.region_idx];
        let conv = Conversation::new("New Chat", model_id, region);
        if !self.ephemeral {
            if let Err(e) = self.db.upsert_conversation(&conv) {
                error!("failed to create conversation: {e:#}");
                self.last_error = Some(format!("DB error: {e:#}"));
                return;
            }
        }
        let id = conv.id.clone();
        self.conversations.insert(0, conv);
        self.select_conversation(&id);
        self.system_prompt_draft.clear();
    }

    fn delete_conversation(&mut self, id: &str) {
        let _ = self.db.delete_conversation(id);
        self.conversations.retain(|c| c.id != id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None; self.messages.clear(); self.md_caches.clear();
            self.conv_usage = TokenUsage::default(); self.last_usage = None;
        }
    }

    fn send_message(&mut self, ctx: &egui::Context) {
        let text = self.input.trim().to_string();
        if text.is_empty() { return; }
        self.last_error = None;

        let conv_id = match &self.active_id {
            Some(id) => id.clone(),
            None => { self.new_conversation(); match &self.active_id { Some(id) => id.clone(), None => return } }
        };

        let user_msg = ChatMessage::new(&conv_id, Role::User, &text);
        if !self.ephemeral { let _ = self.db.insert_message(&user_msg); }
        self.messages.push(user_msg);
        self.input.clear();

        if self.messages.len() == 1 {
            let title: String = text.chars().take(50).collect();
            if let Some(conv) = self.active_conversation_mut() { conv.title = title; conv.updated_at = chrono::Utc::now(); }
            if !self.ephemeral { if let Some(conv) = self.active_conversation() { let _ = self.db.upsert_conversation(conv); } }
        }

        let assistant_msg = ChatMessage::new(&conv_id, Role::Assistant, "");
        if !self.ephemeral { let _ = self.db.insert_message(&assistant_msg); }
        self.messages.push(assistant_msg);

        let history: Vec<(String, String)> = self.messages.iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| (m.role.as_str().to_string(), m.content.clone())).collect();

        let conv_info = self.active_conversation().map(|c| (c.model_id.clone(), c.region.clone(), c.system_prompt.clone()));
        let (model_id, region, system_prompt) = match conv_info { Some(t) => t, None => return };

        self.streaming_md_cache = CommonMarkCache::default();
        let rx = bedrock::spawn_stream(&self.rt, ctx.clone(), model_id, region, system_prompt, history);
        self.stream_rx = Some(rx);
        self.is_streaming = true;
        self.user_scrolled_up = false;
        self.scroll_to_bottom = true;
    }

    fn poll_stream(&mut self) {
        let rx = match &mut self.stream_rx { Some(rx) => rx, None => return };
        loop {
            match rx.try_recv() {
                Ok(StreamToken::Delta(text)) => {
                    if let Some(msg) = self.messages.last_mut() {
                        msg.append_token(&text);
                        if !self.user_scrolled_up { self.scroll_to_bottom = true; }
                    }
                }
                Ok(StreamToken::Done(usage)) => {
                    info!("stream completed");
                    if let Some(u) = usage {
                        self.conv_usage.input_tokens += u.input_tokens;
                        self.conv_usage.output_tokens += u.output_tokens;
                        self.conv_usage.total_tokens += u.total_tokens;
                        self.last_usage = Some(u);
                    }
                    self.finish_stream(); break;
                }
                Ok(StreamToken::Error(e)) => { self.last_error = Some(e); self.finish_stream(); break; }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => { self.finish_stream(); break; }
            }
        }
    }

    fn finish_stream(&mut self) {
        self.is_streaming = false;
        self.stream_rx = None;
        self.user_scrolled_up = false;
        if !self.ephemeral {
            if let Some(msg) = self.messages.last() {
                if msg.role == Role::Assistant { let _ = self.db.update_message_content(&msg.id, &msg.content); }
            }
            if let Some(conv) = self.active_conversation_mut() { conv.updated_at = chrono::Utc::now(); }
            if let Some(conv) = self.active_conversation() { let _ = self.db.upsert_conversation(conv); }
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        if let Some(ref mut cb) = self.clipboard { let _ = cb.set_text(text); }
    }

    // ── Compact context ────────────────────────────────────────────────

    fn compact_context(&mut self, ctx: &egui::Context) {
        if self.messages.is_empty() || self.is_streaming || self.is_compacting { return; }
        let conv_info = self.active_conversation().map(|c| (c.model_id.clone(), c.region.clone()));
        let (model_id, region) = match conv_info { Some(t) => t, None => return };

        let mut prompt = String::from("Summarize the following conversation concisely, preserving all key facts, decisions, and context so it can be used as the starting context for a continuation. Output ONLY the summary, no preamble.\n\n");
        for m in &self.messages {
            if m.content.is_empty() { continue; }
            let role = match m.role { Role::User => "User", Role::Assistant => "Assistant" };
            prompt.push_str(&format!("{role}: {}\n\n", m.content));
        }

        let rx = bedrock::spawn_stream(&self.rt, ctx.clone(), model_id, region, String::new(), vec![("user".into(), prompt)]);
        self.compact_rx = Some(rx);
        self.is_compacting = true;
    }

    fn poll_compact(&mut self) {
        let rx = match &mut self.compact_rx { Some(rx) => rx, None => return };
        let mut summary = String::new();
        let mut done = false;
        loop {
            match rx.try_recv() {
                Ok(StreamToken::Delta(text)) => summary.push_str(&text),
                Ok(StreamToken::Done(_)) => { done = true; break; }
                Ok(StreamToken::Error(e)) => { self.last_error = Some(format!("Compact error: {e}")); done = true; break; }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => { done = true; break; }
            }
        }
        if done {
            self.is_compacting = false;
            self.compact_rx = None;
            if !summary.is_empty() {
                let conv_id = match &self.active_id { Some(id) => id.clone(), None => return };
                self.messages.clear();
                self.md_caches.clear();
                let compacted = ChatMessage::new(&conv_id, Role::Assistant, &format!("[Compacted context]\n\n{summary}"));
                if !self.ephemeral {
                    if let Some(conv) = self.active_conversation() {
                        let _ = self.db.delete_conversation(&conv.id);
                        let conv_clone = conv.clone();
                        let _ = self.db.upsert_conversation(&conv_clone);
                    }
                    let _ = self.db.insert_message(&compacted);
                }
                self.messages.push(compacted);
                self.conv_usage = TokenUsage::default();
                self.scroll_to_bottom = true;
                info!("context compacted");
            }
        }
    }

    // ── Search modal ───────────────────────────────────────────────────

    fn render_search_modal(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        
        // If no search query, show all conversations for browsing
        let items: Vec<(String, String, String)> = if self.search_query.len() >= 2 {
            self.search_results.clone()
        } else if self.search_query.is_empty() {
            // Show recent conversations when no search
            self.conversations.iter().take(20)
                .map(|c| (c.id.clone(), c.title.clone(), String::new()))
                .collect()
        } else {
            Vec::new()
        };
        
        // Arrow key navigation (works even before searching)
        let item_count = items.len();
        if item_count > 0 {
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.search_selected_idx = (self.search_selected_idx + 1).min(item_count - 1);
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.search_selected_idx = self.search_selected_idx.saturating_sub(1);
            }
        }
        
        egui::Area::new(egui::Id::new("search_overlay"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                let screen = ui.ctx().content_rect();
                ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(120));
                egui::Area::new(egui::Id::new("search_modal"))
                    .fixed_pos(egui::pos2(screen.center().x - 250.0, screen.top() + 80.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::new().fill(pal.bg_modal).corner_radius(12.0)
                            .stroke(egui::Stroke::new(1.0, pal.border))
                            .inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                            ui.set_width(500.0);
                            ui.colored_label(pal.text_primary, egui::RichText::new("Search Chats").size(16.0).strong());
                            ui.add_space(8.0);
                            let resp = ui.add(egui::TextEdit::singleline(&mut self.search_query)
                                .desired_width(f32::INFINITY).hint_text("Type to search..."));
                            if resp.changed() {
                                self.search_results = if self.search_query.len() >= 2 {
                                    self.db.search(&self.search_query).unwrap_or_default()
                                } else { Vec::new() };
                                self.search_selected_idx = 0;
                            }
                            // Only grab focus once when the modal first opens
                            if self.search_just_opened {
                                resp.request_focus();
                                self.search_just_opened = false;
                            }
                            
                            ui.add_space(8.0);
                            let mut to_open: Option<String> = None;
                            
                            // Enter to select
                            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && item_count > 0 && self.search_selected_idx < items.len() {
                                to_open = Some(items[self.search_selected_idx].0.clone());
                            }
                            
                            egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, false]).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                
                                if items.is_empty() && self.search_query.len() >= 2 {
                                    ui.colored_label(pal.text_muted, "No results");
                                } else if items.is_empty() && self.search_query.len() == 1 {
                                    ui.colored_label(pal.text_muted, "Type more to search...");
                                }
                                
                                for (idx, (conv_id, title, snippet)) in items.iter().enumerate() {
                                    let is_selected = idx == self.search_selected_idx;
                                    let bg = if is_selected { pal.selected } else { egui::Color32::TRANSPARENT };
                                    
                                    let frame_resp = egui::Frame::new().fill(bg).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.add(egui::Label::new(
                                            egui::RichText::new(title).color(pal.text_primary).size(13.0).strong()
                                        ).selectable(false));
                                        if !snippet.is_empty() {
                                            let snip = if snippet.chars().count() > 80 { snippet.chars().take(77).collect::<String>() + "..." } else { snippet.clone() };
                                            ui.colored_label(pal.text_muted, egui::RichText::new(snip).size(11.5));
                                        }
                                    });
                                    
                                    let row_resp = ui.interact(frame_resp.response.rect, egui::Id::new(("search_row", idx)), egui::Sense::click());
                                    if row_resp.clicked() { to_open = Some(conv_id.clone()); }
                                    if row_resp.hovered() { self.search_selected_idx = idx; }
                                    
                                    ui.add_space(2.0);
                                }
                            });
                            if let Some(id) = to_open { self.select_conversation(&id); self.show_search = false; }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) { self.show_search = false; }
                        });
                    });
            });
    }

    // ── Model picker modal ─────────────────────────────────────────────

    fn render_model_picker(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        
        // Build filtered model list with indices
        let filter = self.model_filter.to_lowercase();
        let filtered: Vec<usize> = MODELS.iter().enumerate()
            .filter(|(_, m)| filter.is_empty() || m.name.to_lowercase().contains(&filter) || m.provider.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        
        // Ensure hover index is valid and set to first if none
        if !filtered.is_empty() {
            if self.model_picker_hover_idx.is_none() || !filtered.contains(&self.model_picker_hover_idx.unwrap()) {
                self.model_picker_hover_idx = Some(filtered[0]);
            }
        }
        
        // Handle keyboard navigation
        if !filtered.is_empty() {
            let current_pos = self.model_picker_hover_idx
                .and_then(|idx| filtered.iter().position(|&i| i == idx))
                .unwrap_or(0);
            
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                let new_pos = (current_pos + 1).min(filtered.len() - 1);
                self.model_picker_hover_idx = Some(filtered[new_pos]);
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                let new_pos = current_pos.saturating_sub(1);
                self.model_picker_hover_idx = Some(filtered[new_pos]);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(idx) = self.model_picker_hover_idx {
                    self.model_idx = idx;
                    self.show_model_picker = false;
                    self.model_filter.clear();
                }
            }
        }
        
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_model_picker = false;
            self.model_filter.clear();
        }
        
        // Click outside to close
        egui::Area::new(egui::Id::new("model_picker_backdrop"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .order(egui::Order::Background)
            .show(ui.ctx(), |ui| {
                let screen = ui.ctx().content_rect();
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                if resp.clicked() {
                    self.show_model_picker = false;
                    self.model_filter.clear();
                }
            });
        
        egui::Area::new(egui::Id::new("model_picker_popup"))
            .fixed_pos(self.model_picker_btn_rect.map(|r| egui::pos2(r.left(), r.bottom() + 4.0)).unwrap_or(egui::pos2(100.0, 60.0)))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new().fill(pal.bg_modal).corner_radius(12.0)
                    .stroke(egui::Stroke::new(1.0, pal.border))
                    .inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    ui.set_width(320.0);
                    
                    let filter_resp = ui.add(egui::TextEdit::singleline(&mut self.model_filter)
                        .hint_text("Filter models...").desired_width(f32::INFINITY));
                    filter_resp.request_focus();
                    
                    ui.add_space(8.0);
                    
                    egui::ScrollArea::vertical().max_height(500.0).auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let mut last_provider = "";
                        for &model_idx in &filtered {
                            let m = &MODELS[model_idx];
                            if m.provider != last_provider {
                                if !last_provider.is_empty() { ui.add_space(6.0); }
                                ui.colored_label(pal.text_muted, egui::RichText::new(m.provider).size(11.0).strong());
                                ui.add_space(2.0);
                                last_provider = m.provider;
                            }
                            
                            let is_selected = self.model_picker_hover_idx == Some(model_idx);
                            let is_current = self.model_idx == model_idx;
                            let bg = if is_selected { pal.selected } else if is_current { pal.hover } else { egui::Color32::TRANSPARENT };
                            
                            let frame_resp = egui::Frame::new().fill(bg).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let text_color = if is_current { pal.accent } else { pal.text_primary };
                                ui.add(egui::Label::new(
                                    egui::RichText::new(m.name).color(text_color).size(13.0)
                                ).selectable(false));
                            });
                            
                            let resp = ui.interact(frame_resp.response.rect, egui::Id::new(("model", model_idx)), egui::Sense::click());
                            if resp.clicked() {
                                self.model_idx = model_idx;
                                self.show_model_picker = false;
                                self.model_filter.clear();
                            }
                            if resp.hovered() {
                                self.model_picker_hover_idx = Some(model_idx);
                            }
                        }
                        
                        if filtered.is_empty() {
                            ui.colored_label(pal.text_muted, "No matching models");
                        }
                    });
                });
            });
    }

    // ── Settings modal ──────────────────────────────────────────────────

    fn render_credentials_modal(&mut self, ui: &mut egui::Ui, is_settings: bool) {
        let pal = self.pal.clone();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, pal.bg_base);

        ui.vertical_centered(|ui| {
            ui.add_space(rect.height() * 0.20);
            egui::Frame::new().inner_margin(egui::Margin::same(32)).corner_radius(16.0)
                .fill(pal.bg_modal).stroke(egui::Stroke::new(1.0, pal.border)).show(ui, |ui| {
                ui.set_width(400.0);
                
                // Header with close button if in settings mode
                ui.horizontal(|ui| {
                    ui.colored_label(pal.text_primary, egui::RichText::new(if is_settings { "Settings" } else { "Bedrock Chat" }).size(24.0).strong());
                    if is_settings {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("x").size(16.0).color(pal.text_muted))
                                .fill(egui::Color32::TRANSPARENT).corner_radius(4.0).min_size(egui::vec2(28.0, 28.0))).clicked() {
                                self.screen = Screen::Chat;
                            }
                        });
                    }
                });
                
                ui.add_space(6.0);
                ui.colored_label(pal.text_secondary, "Paste your Bedrock API key, or skip to use\nyour existing AWS config.");
                ui.add_space(16.0);
                let Screen::Credentials(form) = &mut self.screen else { return; };
                ui.colored_label(pal.text_secondary, "API Key");
                ui.add_space(2.0);
                ui.add(egui::TextEdit::singleline(&mut form.api_key).desired_width(f32::INFINITY).password(true).hint_text("Paste Bedrock API key..."));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.colored_label(pal.text_secondary, "Region");
                    ui.add_space(4.0);
                    egui::ComboBox::from_id_salt("cred_region").selected_text(REGIONS[form.region_idx]).show_ui(ui, |ui| {
                        for (i, region) in REGIONS.iter().enumerate() { ui.selectable_value(&mut form.region_idx, i, *region); }
                    });
                });
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    let Screen::Credentials(form) = &self.screen else { return; };
                    let has_key = !form.api_key.trim().is_empty();
                    if ui.add_enabled(has_key, egui::Button::new(
                        egui::RichText::new("Save").color(if has_key { pal.bg_base } else { pal.text_muted })
                    ).fill(if has_key { pal.accent } else { pal.bg_input }).corner_radius(8.0).min_size(egui::vec2(90.0, 32.0))).clicked() {
                        let Screen::Credentials(form) = &self.screen else { return; };
                        let key = form.api_key.trim().to_string();
                        std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", &key);
                        let _ = self.db.set_config("api_key", &key);
                        let _ = self.db.set_config("region", REGIONS[form.region_idx]);
                        self.region_idx = form.region_idx;
                        self.screen = Screen::Chat;
                    }
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(egui::RichText::new(if is_settings { "Cancel" } else { "Skip" }).color(pal.text_secondary))
                        .fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(8.0).min_size(egui::vec2(70.0, 32.0))).clicked() {
                        let Screen::Credentials(form) = &self.screen else { return; };
                        if !is_settings {
                            let _ = self.db.set_config("region", REGIONS[form.region_idx]);
                            self.region_idx = form.region_idx;
                        }
                        self.screen = Screen::Chat;
                    }
                });
                
                // Delete all chats section (only in settings mode)
                if is_settings {
                    ui.add_space(24.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.colored_label(pal.text_secondary, egui::RichText::new("Danger Zone").size(13.0).strong());
                    ui.add_space(4.0);
                    ui.colored_label(pal.text_muted, egui::RichText::new("This only deletes local chat history.\nNo data is stored on AWS.").size(11.0));
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(egui::RichText::new("Delete All Chats").color(pal.error))
                        .fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, pal.error)).corner_radius(8.0).min_size(egui::vec2(130.0, 32.0))).clicked() {
                        // Delete all conversations
                        let ids: Vec<String> = self.conversations.iter().map(|c| c.id.clone()).collect();
                        for id in ids { let _ = self.db.delete_conversation(&id); }
                        self.conversations.clear();
                        self.active_id = None;
                        self.messages.clear();
                        self.md_caches.clear();
                        self.conv_usage = TokenUsage::default();
                        self.last_usage = None;
                        info!("deleted all chats");
                    }
                }
            });
        });
    }

    // ── Sidebar ────────────────────────────────────────────────────────

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        ui.painter().rect_filled(ui.max_rect(), 0.0, pal.bg_sidebar);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.colored_label(pal.text_primary, egui::RichText::new("Chats").size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                // Settings gear button (use @ as gear substitute)
                if ui.add(egui::Button::new(egui::RichText::new("@").size(14.0).color(pal.text_muted))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0)))
                    .on_hover_text("Settings")
                    .clicked() {
                    self.screen = Screen::Credentials(CredentialForm { api_key: String::new(), region_idx: self.region_idx, is_settings: true });
                }
                // New chat button
                let new_btn = ui.add(egui::Button::new(egui::RichText::new("+").size(16.0).color(pal.accent))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0)))
                    .on_hover_text("New chat");
                if new_btn.clicked() {
                    self.burst.spawn(new_btn.rect.center(), 20);
                    self.new_conversation();
                }
                // Ephemeral toggle - creates new chat only when turning on AND current chat has messages
                let eph_label = if self.ephemeral { "~" } else { "~" };
                let eph_btn = ui.add(egui::Button::new(egui::RichText::new(eph_label).size(14.0)
                    .color(if self.ephemeral { pal.accent } else { pal.text_muted }))
                    .fill(if self.ephemeral { pal.selected } else { egui::Color32::TRANSPARENT })
                    .corner_radius(6.0).min_size(egui::vec2(28.0, 28.0)))
                    .on_hover_text(if self.ephemeral { "Ephemeral ON (click to exit)" } else { "New ephemeral chat" });
                if eph_btn.clicked() {
                    if !self.ephemeral {
                        // Turning ON ephemeral - only create new chat if current has messages
                        self.ephemeral = true;
                        if self.messages.is_empty() && self.active_id.is_some() {
                            // Already in empty chat, just mark it ephemeral (it won't be saved)
                        } else {
                            self.burst.spawn(eph_btn.rect.center(), 20);
                            self.new_conversation();
                        }
                    } else {
                        // Turning OFF ephemeral
                        self.ephemeral = false;
                    }
                }
                // Search button
                if ui.add(egui::Button::new(egui::RichText::new("/").size(14.0).color(pal.text_muted))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0)))
                    .on_hover_text("Search chats (Cmd+K)")
                    .clicked() {
                    self.show_search = true; self.search_just_opened = true; self.search_query.clear(); self.search_results.clear();
                    self.search_selected_idx = 0;
                }
            });
        });
        ui.add_space(4.0);
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([r.left_top(), egui::pos2(r.right(), r.top())], egui::Stroke::new(1.0, pal.border));
        ui.add_space(6.0);

        let active_id = self.active_id.clone();
        let mut to_select: Option<String> = None;
        let mut to_delete: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for conv in &self.conversations {
                let is_active = active_id.as_deref() == Some(&conv.id);
                let conv_id = conv.id.clone();
                let title: String = if conv.title.chars().count() > 28 { conv.title.chars().take(25).collect::<String>() + "..." } else { conv.title.clone() };
                
                let row_rect = ui.available_rect_before_wrap();
                let is_hovered = ui.rect_contains_pointer(egui::Rect::from_min_size(
                    row_rect.min, egui::vec2(ui.available_width(), 32.0)
                ));
                let bg = if is_active { pal.selected } else if is_hovered { pal.hover } else { egui::Color32::TRANSPARENT };
                
                let frame_resp = egui::Frame::new().fill(bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).outer_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let tc = if is_active { pal.text_primary } else { pal.text_secondary };
                        ui.add(egui::Label::new(egui::RichText::new(&title).color(tc).size(13.0)).selectable(false));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Always show button but only visible when hovered/active
                            let btn_color = if is_active || is_hovered { pal.text_muted } else { egui::Color32::TRANSPARENT };
                            let del_btn = ui.add(egui::Button::new(egui::RichText::new("x").color(btn_color).size(12.0))
                                .fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(20.0, 20.0)));
                            if del_btn.clicked() {
                                to_delete = Some(conv_id.clone());
                            }
                        });
                    });
                });
                
                // Only handle row click if delete wasn't clicked
                if to_delete.is_none() {
                    let row_resp = ui.interact(frame_resp.response.rect, egui::Id::new(("conv_row", &conv_id)), egui::Sense::click());
                    if row_resp.clicked() && !is_active {
                        to_select = Some(conv_id);
                    }
                }
            }
        });
        if let Some(id) = to_delete { self.delete_conversation(&id); }
        else if let Some(id) = to_select { self.select_conversation(&id); }
    }

    // ── Chat pane ──────────────────────────────────────────────────────

    fn render_chat_pane(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        let full_rect = ui.max_rect();
        ui.painter().rect_filled(full_rect, 0.0, pal.bg_base);

        // Draw faint ghost for ephemeral mode
        if self.ephemeral {
            let center = full_rect.center();
            let ghost_color = egui::Color32::from_rgba_unmultiplied(
                pal.text_muted.r(), pal.text_muted.g(), pal.text_muted.b(), 15
            );
            // Simple ghost shape: head circle + body
            let head_center = egui::pos2(center.x, center.y - 30.0);
            ui.painter().circle_filled(head_center, 40.0, ghost_color);
            // Body (rounded rect below)
            let body_rect = egui::Rect::from_center_size(
                egui::pos2(center.x, center.y + 30.0),
                egui::vec2(80.0, 70.0)
            );
            ui.painter().rect_filled(body_rect, 20.0, ghost_color);
            // Eyes
            let eye_color = egui::Color32::from_rgba_unmultiplied(
                pal.bg_base.r(), pal.bg_base.g(), pal.bg_base.b(), 30
            );
            ui.painter().circle_filled(egui::pos2(center.x - 15.0, center.y - 35.0), 8.0, eye_color);
            ui.painter().circle_filled(egui::pos2(center.x + 15.0, center.y - 35.0), 8.0, eye_color);
            // Wavy bottom (3 bumps)
            for i in 0..3 {
                let bx = center.x - 30.0 + (i as f32 * 30.0);
                ui.painter().circle_filled(egui::pos2(bx, center.y + 65.0), 15.0, ghost_color);
            }
        }

        // Draw any active burst particles
        let dt = self.get_dt(ui);
        if self.burst.draw(ui.painter(), &pal, dt) {
            ui.ctx().request_repaint(); // only repaint while burst is active
        }

        if self.active_id.is_none() {
            ui.centered_and_justified(|ui| { ui.colored_label(pal.text_muted, "Select or create a conversation"); });
            return;
        }

        self.render_top_bar(ui);
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([r.left_top(), egui::pos2(r.right(), r.top())], egui::Stroke::new(1.0, pal.border));
        ui.add_space(2.0);

        let input_h = 100.0;
        let avail = ui.available_height() - input_h;
        ui.allocate_ui(egui::vec2(ui.available_width(), avail.max(100.0)), |ui| { self.render_messages(ui); });

        if let Some(err) = self.last_error.clone() {
            ui.horizontal(|ui| {
                ui.add_space(16.0); ui.colored_label(pal.error, &err);
                if ui.small_button("dismiss").clicked() { self.last_error = None; }
            });
        }
        if self.is_compacting {
            ui.horizontal(|ui| { ui.add_space(16.0); ui.spinner(); ui.colored_label(pal.text_muted, "Compacting context..."); });
        }

        self.render_input(ui);
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        let has_msgs = self.conv_has_messages();
        egui::Frame::new().fill(pal.bg_topbar).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(pal.text_secondary, "Model");
                ui.add_space(2.0);
                let current_name = MODELS[self.model_idx].name;
                
                // Custom model picker button (use simple "v" for caret)
                let btn_resp = ui.add(egui::Button::new(
                    egui::RichText::new(format!("{}  v", current_name)).color(pal.text_primary)
                ).fill(pal.bg_input).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(6.0).min_size(egui::vec2(220.0, 24.0)));
                
                // Store button rect for popup positioning
                self.model_picker_btn_rect = Some(btn_resp.rect);
                
                if btn_resp.clicked() {
                    self.show_model_picker = !self.show_model_picker;
                    if self.show_model_picker {
                        self.model_filter.clear();
                        self.model_picker_hover_idx = Some(self.model_idx);
                    }
                }

                ui.add_space(8.0);
                ui.colored_label(pal.text_secondary, "Region");
                ui.add_space(2.0);
                if has_msgs {
                    ui.colored_label(pal.text_muted, REGIONS[self.region_idx]);
                } else {
                    egui::ComboBox::from_id_salt("region_picker").selected_text(REGIONS[self.region_idx]).show_ui(ui, |ui| {
                        for (i, region) in REGIONS.iter().enumerate() { ui.selectable_value(&mut self.region_idx, i, *region); }
                    });
                }

                ui.add_space(8.0);
                // System prompt button with preview
                let sys_prompt = self.active_conversation().map(|c| c.system_prompt.clone()).unwrap_or_default();
                let has_sys_prompt = !sys_prompt.is_empty();
                ui.vertical(|ui| {
                    if ui.selectable_label(self.show_system_prompt, "System Prompt").clicked() {
                        self.show_system_prompt = !self.show_system_prompt;
                        if self.show_system_prompt {
                            self.system_prompt_draft = sys_prompt.clone();
                        }
                    }
                    // Show preview of system prompt if set
                    if has_sys_prompt && !self.show_system_prompt {
                        let preview: String = sys_prompt.chars().take(30).collect();
                        let preview = if sys_prompt.len() > 30 { format!("{}...", preview) } else { preview };
                        ui.colored_label(pal.text_muted, egui::RichText::new(preview).size(10.0));
                    }
                });

                // Compact button
                if has_msgs && !self.is_streaming && !self.is_compacting {
                    ui.add_space(4.0);
                    if ui.small_button("Compact").clicked() {
                        let ctx = ui.ctx().clone();
                        self.compact_context(&ctx);
                    }
                }

                if self.conv_usage.total_tokens > 0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(pal.text_muted, egui::RichText::new(
                            format!("{}in / {}out", self.conv_usage.input_tokens, self.conv_usage.output_tokens)
                        ).size(11.0));
                    });
                }
            });
        });

        let model_id = MODELS[self.model_idx].id.to_string();
        let region = REGIONS[self.region_idx].to_string();
        if let Some(conv) = self.active_conversation_mut() {
            if conv.model_id != model_id || conv.region != region { conv.model_id = model_id; conv.region = region; }
        }
        if !self.ephemeral { if let Some(conv) = self.active_conversation() { let _ = self.db.upsert_conversation(conv); } }

        if self.show_system_prompt {
            egui::Frame::new().fill(pal.bg_topbar).inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized(egui::vec2(ui.available_width() - 70.0, 50.0),
                        egui::TextEdit::multiline(&mut self.system_prompt_draft).hint_text("Enter system prompt..."));
                    let current = self.active_conversation().map(|c| c.system_prompt.clone()).unwrap_or_default();
                    let changed = self.system_prompt_draft != current;
                    if ui.add_enabled(changed, egui::Button::new(
                        egui::RichText::new("Set").color(if changed { egui::Color32::WHITE } else { pal.text_muted })
                    ).fill(if changed { pal.accent } else { pal.bg_input }).corner_radius(6.0).min_size(egui::vec2(50.0, 28.0))).clicked() {
                        let draft = self.system_prompt_draft.clone();
                        if let Some(conv) = self.active_conversation_mut() { conv.system_prompt = draft; }
                        if !self.ephemeral { if let Some(conv) = self.active_conversation() { let _ = self.db.upsert_conversation(conv); } }
                        self.show_system_prompt = false; // Close after setting
                    }
                });
            });
        }
    }

    fn render_messages(&mut self, ui: &mut egui::Ui) {
        let output = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(!self.user_scrolled_up)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let side_pad = (ui.available_width() * 0.04).clamp(12.0, 40.0);
                ui.add_space(8.0);
                let n = self.messages.len();
                for i in 0..n {
                    let streaming = i == n - 1 && self.is_streaming;
                    ui.horizontal(|ui| {
                        ui.add_space(side_pad);
                        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                            ui.set_width(ui.available_width() - side_pad);
                            self.render_single_message(ui, i, streaming);
                        });
                    });
                }
                if self.scroll_to_bottom {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    self.scroll_to_bottom = false;
                }
            });

        // Detect user scrolling up during streaming
        if self.is_streaming {
            let max_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
            let current_offset = output.state.offset.y;
            let at_bottom = max_offset < 1.0 || (max_offset - current_offset) < 20.0;

            if !at_bottom {
                self.user_scrolled_up = true;
            } else if self.user_scrolled_up {
                // User scrolled back to bottom -- re-enable auto-scroll
                self.user_scrolled_up = false;
            }
        }
    }

    fn render_single_message(&mut self, ui: &mut egui::Ui, idx: usize, is_streaming: bool) {
        let pal = self.pal.clone();
        let role = self.messages[idx].role;
        let (rl, rc, bg) = match role {
            Role::User => ("You", pal.role_user, pal.bg_user_msg),
            Role::Assistant => ("Assistant", pal.role_assistant, pal.bg_assist_msg),
        };
        let empty = self.messages[idx].content.is_empty();
        let copy_text = if role == Role::Assistant && !empty { Some(self.messages[idx].content.clone()) } else { None };

        egui::Frame::new().fill(bg).corner_radius(10.0).inner_margin(egui::Margin::symmetric(16, 12)).outer_margin(egui::Margin::symmetric(0, 3)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.colored_label(rc, egui::RichText::new(rl).size(12.5).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(ref t) = copy_text {
                        if ui.add(egui::Button::new(egui::RichText::new("Copy").size(11.0).color(pal.text_muted)).fill(egui::Color32::TRANSPARENT).corner_radius(4.0)).clicked() { self.copy_to_clipboard(t); }
                    }
                    if is_streaming { ui.spinner(); }
                });
            });
            ui.add_space(6.0);
            if empty && is_streaming { ui.colored_label(pal.text_muted, "..."); }
            else if !empty {
                let content = self.messages[idx].content.clone();
                if is_streaming { CommonMarkViewer::new().show(ui, &mut self.streaming_md_cache, &content); }
                else {
                    let mid = self.messages[idx].id.clone();
                    let v = self.messages[idx].version;
                    let e = self.md_caches.entry(mid).or_insert_with(|| (v, CommonMarkCache::default()));
                    if e.0 != v { *e = (v, CommonMarkCache::default()); }
                    CommonMarkViewer::new().show(ui, &mut e.1, &content);
                }
            }
        });
    }

    fn render_input(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();

        egui::Frame::new().fill(egui::Color32::TRANSPARENT).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
            egui::Frame::new().fill(pal.bg_input).corner_radius(12.0).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let rows = (self.input.chars().filter(|c| *c == '\n').count() + 1).clamp(1, 8);
                    let resp = ui.add_sized(egui::vec2(ui.available_width() - 60.0, 0.0),
                        egui::TextEdit::multiline(&mut self.input).hint_text(egui::RichText::new("Message...").color(pal.text_muted))
                            .desired_rows(rows).lock_focus(true).text_color(pal.text_primary));
                    
                    // Enter sends, Shift+Enter for newline
                    let enter_pressed = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let shift_held = ui.input(|i| i.modifiers.shift);
                    let should_send = enter_pressed && !shift_held;
                    
                    // Strip trailing newline that egui adds on Enter
                    if should_send && self.input.ends_with('\n') {
                        self.input.pop();
                    }
                    
                    let can = !self.is_streaming && !self.is_compacting && !self.input.trim().is_empty();
                    let bc = if can { pal.accent } else { pal.accent_dim };
                    let text_color = if can { egui::Color32::WHITE } else { pal.text_muted };
                    let clicked = ui.add(egui::Button::new(egui::RichText::new("Send").color(text_color).size(13.0)).fill(bc).corner_radius(8.0).min_size(egui::vec2(52.0, 30.0))).clicked();
                    if (should_send || clicked) && can { let ctx = ui.ctx().clone(); self.send_message(&ctx); }
                });
            });
        });
    }
}

// ── Visuals ─────────────────────────────────────────────────────────────

fn apply_visuals(ctx: &egui::Context, theme: egui::Theme, pal: &Palette) {
    let mut v = theme.default_visuals();
    v.panel_fill = pal.bg_base; v.window_fill = pal.bg_base;
    v.extreme_bg_color = pal.bg_input; v.faint_bg_color = pal.bg_sidebar;
    v.widgets.noninteractive.bg_fill = pal.bg_input;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, pal.text_primary);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, pal.border);
    v.widgets.inactive.bg_fill = pal.bg_input;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, pal.text_secondary);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, pal.border);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.bg_fill = pal.hover;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, pal.text_primary);
    v.widgets.active.bg_fill = pal.selected;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, pal.text_primary);
    v.selection.bg_fill = pal.accent_dim;
    v.selection.stroke = egui::Stroke::new(1.0, pal.text_primary);
    ctx.set_visuals(v);
    let mut s = (*ctx.global_style()).clone();
    s.spacing.item_spacing = egui::vec2(6.0, 4.0);
    s.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_global_style(s);
}

// ── eframe::App ─────────────────────────────────────────────────────────

impl eframe::App for ChatApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.pal.bg_base;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.check_theme(ui.ctx());
        self.poll_compact();

        // Double-tap Escape to interrupt streaming
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !self.show_search && !self.show_model_picker {
            let now = ui.input(|i| i.time);
            if (self.is_streaming || self.is_compacting) && (now - self.last_escape_time) < 0.4 {
                // Second tap within 400ms -- cancel
                if self.is_streaming {
                    self.stream_rx = None;
                    self.is_streaming = false;
                    self.user_scrolled_up = false;
                    // Persist whatever we have so far
                    if !self.ephemeral {
                        if let Some(msg) = self.messages.last() {
                            if msg.role == Role::Assistant && !msg.content.is_empty() {
                                let _ = self.db.update_message_content(&msg.id, &msg.content);
                            }
                        }
                    }
                    info!("stream interrupted by user");
                }
                if self.is_compacting {
                    self.compact_rx = None;
                    self.is_compacting = false;
                    info!("compact interrupted by user");
                }
                self.last_escape_time = 0.0;
            } else {
                self.last_escape_time = now;
            }
        }

        // Cmd+K shortcut for search
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::K)) {
            self.show_search = !self.show_search;
            if self.show_search { self.search_just_opened = true; self.search_query.clear(); self.search_results.clear(); }
        }

        match &self.screen {
            Screen::Credentials(ref form) => { 
                let is_settings = form.is_settings;
                self.render_credentials_modal(ui, is_settings); 
            }
            Screen::Chat => {
                self.poll_stream();
                egui::Panel::left("sidebar").default_size(240.0).min_size(180.0)
                    .show_inside(ui, |ui| { self.render_sidebar(ui); });
                egui::CentralPanel::default().show_inside(ui, |ui| { self.render_chat_pane(ui); });
            }
        }

        if self.show_search { self.render_search_modal(ui); }
        if self.show_model_picker { self.render_model_picker(ui); }
    }
}
