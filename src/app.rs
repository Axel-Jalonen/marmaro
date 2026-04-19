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

// ── Particle system ────────────────────────────────────────────────────
// Particles drift, breathe, and draw faint connections to nearby neighbors.
// Mouse proximity causes them to gently scatter.

struct Particle {
    x: f32, y: f32,        // normalized 0..1
    vx: f32, vy: f32,
    base_vx: f32, base_vy: f32,  // original velocity (restored after mouse scatter)
    radius: f32,
    base_alpha: f32,
    color_idx: u8,          // 0=accent, 1=accent2, 2=accent3
    phase: f32,             // unique phase offset for breathing
    depth: f32,             // 0..1 parallax layer (0=far, 1=near)
}

struct Particles {
    list: Vec<Particle>,
    time: f64,
}

impl Particles {
    fn new(count: usize) -> Self {
        let mut rng = rand::thread_rng();
        Self {
            list: (0..count).map(|_| {
                let depth = rng.gen_range(0.2_f32..1.0);
                let speed = 0.002 + depth * 0.004;
                let vx = rng.gen_range(-speed..speed);
                let vy = rng.gen_range(-speed..speed);
                Particle {
                    x: rng.gen_range(0.0..1.0), y: rng.gen_range(0.0..1.0),
                    vx, vy, base_vx: vx, base_vy: vy,
                    radius: 1.0 + depth * 3.0,
                    base_alpha: 0.1 + depth * 0.35,
                    color_idx: rng.gen_range(0..3),
                    phase: rng.gen_range(0.0..std::f32::consts::TAU),
                    depth,
                }
            }).collect(),
            time: 0.0,
        }
    }

    fn draw(&mut self, painter: &egui::Painter, rect: egui::Rect, pal: &Palette, dt: f32, mouse: Option<egui::Pos2>) {
        self.time += dt as f64;
        let t = self.time as f32;

        // Update positions + mouse interaction
        for p in &mut self.list {
            // Mouse repulsion
            if let Some(mpos) = mouse {
                let mx = (mpos.x - rect.left()) / rect.width();
                let my = (mpos.y - rect.top()) / rect.height();
                let dx = p.x - mx;
                let dy = p.y - my;
                let dist_sq = dx * dx + dy * dy;
                let repel_radius = 0.04; // normalized
                if dist_sq < repel_radius && dist_sq > 0.0001 {
                    let dist = dist_sq.sqrt();
                    let force = ((repel_radius - dist) / repel_radius) * 0.02 * p.depth;
                    p.vx += (dx / dist) * force;
                    p.vy += (dy / dist) * force;
                }
            }

            // Dampen back toward base velocity
            p.vx += (p.base_vx - p.vx) * 0.02;
            p.vy += (p.base_vy - p.vy) * 0.02;

            p.x += p.vx * dt;
            p.y += p.vy * dt;
            if p.x < -0.02 { p.x += 1.04; } if p.x > 1.02 { p.x -= 1.04; }
            if p.y < -0.02 { p.y += 1.04; } if p.y > 1.02 { p.y -= 1.04; }
        }

        // Draw connection lines between nearby particles (only near-layer ones)
        let connect_dist = 0.12_f32;
        let connect_dist_sq = connect_dist * connect_dist;
        let n = self.list.len();
        for i in 0..n {
            if self.list[i].depth < 0.5 { continue; } // skip far particles
            for j in (i + 1)..n {
                if self.list[j].depth < 0.5 { continue; }
                let dx = self.list[i].x - self.list[j].x;
                let dy = self.list[i].y - self.list[j].y;
                let d2 = dx * dx + dy * dy;
                if d2 < connect_dist_sq {
                    let closeness = 1.0 - (d2 / connect_dist_sq);
                    let alpha = (closeness * 25.0 * self.list[i].depth * self.list[j].depth) as u8;
                    if alpha > 0 {
                        let col = Self::particle_color(pal, self.list[i].color_idx);
                        let line_color = egui::Color32::from_rgba_premultiplied(col.r(), col.g(), col.b(), alpha);
                        let p1 = egui::pos2(
                            rect.left() + self.list[i].x * rect.width(),
                            rect.top() + self.list[i].y * rect.height(),
                        );
                        let p2 = egui::pos2(
                            rect.left() + self.list[j].x * rect.width(),
                            rect.top() + self.list[j].y * rect.height(),
                        );
                        painter.line_segment([p1, p2], egui::Stroke::new(0.5, line_color));
                    }
                }
            }
        }

        // Draw particles
        for p in &self.list {
            let sx = rect.left() + p.x * rect.width();
            let sy = rect.top() + p.y * rect.height();

            // Breathing: slow sine pulse on alpha
            let breath = ((t * 0.4 + p.phase).sin() * 0.5 + 0.5) * 0.6 + 0.4;
            let a = (p.base_alpha * breath * 255.0) as u8;

            let base_col = Self::particle_color(pal, p.color_idx);
            let color = egui::Color32::from_rgba_premultiplied(base_col.r(), base_col.g(), base_col.b(), a);

            // Outer glow (larger, more transparent)
            if p.depth > 0.6 {
                let glow_a = (a as f32 * 0.25) as u8;
                let glow_col = egui::Color32::from_rgba_premultiplied(base_col.r(), base_col.g(), base_col.b(), glow_a);
                painter.circle_filled(egui::pos2(sx, sy), p.radius * 2.5, glow_col);
            }

            painter.circle_filled(egui::pos2(sx, sy), p.radius, color);
        }
    }

    fn particle_color(pal: &Palette, idx: u8) -> egui::Color32 {
        match idx {
            0 => pal.accent,
            1 => pal.accent2,
            _ => pal.accent3,
        }
    }
}

// ── App state ──────────────────────────────────────────────────────────

#[derive(Default)]
struct CredentialForm { api_key: String, region_idx: usize }

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
    model_idx: usize,
    region_idx: usize,
    show_system_prompt: bool,
    clipboard: Option<arboard::Clipboard>,

    conv_usage: TokenUsage,
    last_usage: Option<TokenUsage>,

    current_theme: egui::Theme,
    pal: Palette,
    particles: Particles,
    last_frame_time: Option<f64>,

    model_filter: String,
    ephemeral: bool,

    show_search: bool,
    search_query: String,
    search_results: Vec<(String, String, String)>,

    is_compacting: bool,
    compact_rx: Option<mpsc::UnboundedReceiver<StreamToken>>,
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
            scroll_to_bottom: false, model_idx: 0, region_idx: saved_region,
            show_system_prompt: false, clipboard, conv_usage: TokenUsage::default(),
            last_usage: None, current_theme: theme, pal, particles: Particles::new(60),
            last_frame_time: None, model_filter: String::new(), ephemeral: false,
            show_search: false, search_query: String::new(), search_results: Vec::new(),
            is_compacting: false, compact_rx: None,
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

    fn get_dt_and_mouse(&mut self, ui: &egui::Ui) -> (f32, Option<egui::Pos2>) {
        let now = ui.input(|i| i.time);
        let dt = self.last_frame_time.map_or(0.016, |t| (now - t) as f32).min(0.1);
        self.last_frame_time = Some(now);
        let mouse = ui.input(|i| i.pointer.hover_pos());
        (dt, mouse)
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
        self.active_id = Some(id.to_string());
        self.conv_usage = TokenUsage::default();
        self.last_usage = None;
        match self.db.list_messages(id) {
            Ok(msgs) => { self.messages = msgs; self.md_caches.clear(); self.scroll_to_bottom = true; }
            Err(e) => { error!("failed to load messages: {e:#}"); self.last_error = Some(format!("Failed to load: {e:#}")); }
        }
        let conv_data = self.active_conversation().map(|c| (c.model_id.clone(), c.region.clone()));
        if let Some((model_id, region)) = conv_data {
            if let Some(idx) = MODELS.iter().position(|m| m.id == model_id) { self.model_idx = idx; }
            if let Some(idx) = REGIONS.iter().position(|r| *r == region) { self.region_idx = idx; }
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
        self.scroll_to_bottom = true;
    }

    fn poll_stream(&mut self) {
        let rx = match &mut self.stream_rx { Some(rx) => rx, None => return };
        loop {
            match rx.try_recv() {
                Ok(StreamToken::Delta(text)) => {
                    if let Some(msg) = self.messages.last_mut() { msg.append_token(&text); self.scroll_to_bottom = true; }
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
                            }
                            resp.request_focus();
                            ui.add_space(8.0);
                            let mut to_open: Option<String> = None;
                            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                if self.search_results.is_empty() && self.search_query.len() >= 2 {
                                    ui.colored_label(pal.text_muted, "No results");
                                }
                                for (conv_id, title, snippet) in &self.search_results {
                                    let r = ui.add(egui::Label::new(
                                        egui::RichText::new(title).color(pal.text_primary).size(13.0).strong()
                                    ).selectable(false).sense(egui::Sense::click()));
                                    let snip = if snippet.chars().count() > 80 { snippet.chars().take(77).collect::<String>() + "..." } else { snippet.clone() };
                                    ui.colored_label(pal.text_muted, egui::RichText::new(snip).size(11.5));
                                    ui.add_space(4.0);
                                    if r.clicked() { to_open = Some(conv_id.clone()); }
                                }
                            });
                            if let Some(id) = to_open { self.select_conversation(&id); self.show_search = false; }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) { self.show_search = false; }
                        });
                    });
            });
    }

    // ── Credential modal ───────────────────────────────────────────────

    fn render_credentials_modal(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, pal.bg_base);
        let (dt, mouse) = self.get_dt_and_mouse(ui);
        self.particles.draw(ui.painter(), rect, &pal, dt, mouse);

        ui.vertical_centered(|ui| {
            ui.add_space(rect.height() * 0.28);
            egui::Frame::new().inner_margin(egui::Margin::same(32)).corner_radius(16.0)
                .fill(pal.bg_modal).stroke(egui::Stroke::new(1.0, pal.border)).show(ui, |ui| {
                ui.set_width(400.0);
                ui.colored_label(pal.text_primary, egui::RichText::new("Bedrock Chat").size(24.0).strong());
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
                        egui::RichText::new("Connect").color(if has_key { pal.bg_base } else { pal.text_muted })
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
                    if ui.add(egui::Button::new(egui::RichText::new("Skip").color(pal.text_secondary))
                        .fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(8.0).min_size(egui::vec2(70.0, 32.0))).clicked() {
                        let Screen::Credentials(form) = &self.screen else { return; };
                        let _ = self.db.set_config("region", REGIONS[form.region_idx]);
                        self.region_idx = form.region_idx;
                        self.screen = Screen::Chat;
                    }
                });
            });
        });
        ui.ctx().request_repaint();
    }

    // ── Sidebar ────────────────────────────────────────────────────────

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        ui.painter().rect_filled(ui.max_rect(), 0.0, pal.bg_sidebar);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.colored_label(pal.text_primary, egui::RichText::new("Chats").size(16.0).strong());
            if self.ephemeral {
                ui.colored_label(pal.accent, egui::RichText::new("ephemeral").size(10.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                // Settings gear button
                if ui.add(egui::Button::new(egui::RichText::new("\u{2699}").size(16.0).color(pal.text_muted))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0))).clicked() {
                    self.screen = Screen::Credentials(CredentialForm { api_key: String::new(), region_idx: self.region_idx });
                }
                // New chat button
                if ui.add(egui::Button::new(egui::RichText::new("+").size(16.0).color(pal.accent))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0))).clicked() {
                    self.new_conversation();
                }
                // Search button
                if ui.add(egui::Button::new(egui::RichText::new("\u{1F50D}").size(13.0).color(pal.text_muted))
                    .fill(egui::Color32::TRANSPARENT).corner_radius(6.0).min_size(egui::vec2(28.0, 28.0))).clicked() {
                    self.show_search = true; self.search_query.clear(); self.search_results.clear();
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
                let bg = if is_active { pal.selected } else { egui::Color32::TRANSPARENT };
                let title: String = if conv.title.chars().count() > 28 { conv.title.chars().take(25).collect::<String>() + "..." } else { conv.title.clone() };
                egui::Frame::new().fill(bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).outer_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let tc = if is_active { pal.text_primary } else { pal.text_secondary };
                        let resp = ui.add(egui::Label::new(egui::RichText::new(&title).color(tc).size(13.0)).selectable(false).sense(egui::Sense::click()));
                        if resp.clicked() && !is_active { to_select = Some(conv.id.clone()); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_active || ui.rect_contains_pointer(ui.max_rect()) {
                                if ui.add(egui::Button::new(egui::RichText::new("x").color(pal.text_muted).size(12.0)).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(20.0, 20.0))).clicked() {
                                    to_delete = Some(conv.id.clone());
                                }
                            }
                        });
                    });
                });
            }
        });
        if let Some(id) = to_select { self.select_conversation(&id); }
        if let Some(id) = to_delete { self.delete_conversation(&id); }
    }

    // ── Chat pane ──────────────────────────────────────────────────────

    fn render_chat_pane(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        let full_rect = ui.max_rect();
        ui.painter().rect_filled(full_rect, 0.0, pal.bg_base);
        let (dt, mouse) = self.get_dt_and_mouse(ui);
        self.particles.draw(ui.painter(), full_rect, &pal, dt, mouse);

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
        ui.ctx().request_repaint();
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal.clone();
        let has_msgs = self.conv_has_messages();
        egui::Frame::new().fill(pal.bg_topbar).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(pal.text_secondary, "Model");
                ui.add_space(2.0);
                let current_name = MODELS[self.model_idx].name;
                egui::ComboBox::from_id_salt("model_picker").selected_text(current_name).width(220.0).show_ui(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.model_filter).hint_text("Filter models...").desired_width(200.0));
                    ui.add_space(4.0);
                    let filter = self.model_filter.to_lowercase();
                    let mut last_provider = "";
                    for (i, m) in MODELS.iter().enumerate() {
                        if !filter.is_empty() && !m.name.to_lowercase().contains(&filter) && !m.provider.to_lowercase().contains(&filter) { continue; }
                        if m.provider != last_provider {
                            if !last_provider.is_empty() { ui.separator(); }
                            ui.colored_label(pal.text_muted, egui::RichText::new(m.provider).size(11.0).strong());
                            last_provider = m.provider;
                        }
                        ui.selectable_value(&mut self.model_idx, i, m.name);
                    }
                });

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
                if ui.selectable_label(self.show_system_prompt, "System Prompt").clicked() {
                    self.show_system_prompt = !self.show_system_prompt;
                }

                // Ephemeral toggle
                ui.add_space(4.0);
                if ui.selectable_label(self.ephemeral, "Ephemeral").clicked() {
                    self.ephemeral = !self.ephemeral;
                }

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
                let mut sys = self.active_conversation().map(|c| c.system_prompt.clone()).unwrap_or_default();
                if ui.add(egui::TextEdit::multiline(&mut sys).hint_text("Enter system prompt...").desired_rows(2).desired_width(f32::INFINITY)).changed() {
                    if let Some(conv) = self.active_conversation_mut() { conv.system_prompt = sys; }
                    if !self.ephemeral { if let Some(conv) = self.active_conversation() { let _ = self.db.upsert_conversation(conv); } }
                }
            });
        }
    }

    fn render_messages(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
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
            if self.scroll_to_bottom { ui.scroll_to_cursor(Some(egui::Align::BOTTOM)); self.scroll_to_bottom = false; }
        });
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
        let sc = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
        let sc2 = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Enter);

        egui::Frame::new().fill(egui::Color32::TRANSPARENT).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
            egui::Frame::new().fill(pal.bg_input).corner_radius(12.0).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let rows = (self.input.chars().filter(|c| *c == '\n').count() + 1).clamp(1, 8);
                    let resp = ui.add_sized(egui::vec2(ui.available_width() - 60.0, 0.0),
                        egui::TextEdit::multiline(&mut self.input).hint_text(egui::RichText::new("Message...").color(pal.text_muted))
                            .desired_rows(rows).lock_focus(true).text_color(pal.text_primary));
                    let ce = ui.input_mut(|i| i.consume_shortcut(&sc) || i.consume_shortcut(&sc2));
                    let can = !self.is_streaming && !self.is_compacting && !self.input.trim().is_empty();
                    let bc = if can { pal.accent } else { pal.accent_dim };
                    let clicked = ui.add(egui::Button::new(egui::RichText::new("Send").color(if can { pal.bg_base } else { pal.text_muted }).size(13.0)).fill(bc).corner_radius(8.0).min_size(egui::vec2(52.0, 30.0))).clicked();
                    if (ce || clicked) && can { let ctx = ui.ctx().clone(); self.send_message(&ctx); }
                    if !self.is_streaming { resp.request_focus(); }
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

        // Cmd+K shortcut for search
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::K)) {
            self.show_search = !self.show_search;
            if self.show_search { self.search_query.clear(); self.search_results.clear(); }
        }

        match &self.screen {
            Screen::Credentials(_) => { self.render_credentials_modal(ui); }
            Screen::Chat => {
                self.poll_stream();
                egui::Panel::left("sidebar").default_size(240.0).min_size(180.0)
                    .show_inside(ui, |ui| { self.render_sidebar(ui); });
                egui::CentralPanel::default().show_inside(ui, |ui| { self.render_chat_pane(ui); });
            }
        }

        if self.show_search { self.render_search_modal(ui); }
    }
}
