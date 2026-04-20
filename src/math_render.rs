//! Math rendering via ReX (LaTeX parser + layout engine) into egui.
//!
//! This module provides:
//! - `render_math_ui`: renders a LaTeX string as laid-out math in an egui `Ui`
//! - `parse_math_segments`: splits markdown content into text and math segments

use eframe::egui;
use rex::layout::Style;
use rex::render::{Cursor, RenderSettings, Renderer};
use rex::dimensions::Float;
use rex::parser::color::RGBA;
use rex::fp::F24P8 as FontUnit;

// ── Constants ───────────────────────────────────────────────────────────

/// UNITS_PER_EM for the STIX2 font embedded in ReX.
const UNITS_PER_EM: f64 = 1000.0;

// ── Draw commands ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum DrawCmd {
    /// Draw a single unicode glyph at (x, y) with given scale.
    Glyph {
        x: f64,
        y: f64,
        codepoint: u32,
        scale: f64,
    },
    /// Draw a filled rectangle (fraction bars, radical bars, etc.).
    Rule {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// Push a color onto the color stack.
    PushColor(RGBA),
    /// Pop the color stack.
    PopColor,
}

// ── EguiRenderer ────────────────────────────────────────────────────────

struct EguiRenderer {
    settings: RenderSettings,
}

impl EguiRenderer {
    fn new(font_size: u16, display: bool) -> Self {
        let style = if display { Style::Display } else { Style::Text };
        let settings = RenderSettings::default()
            .font_size(font_size)
            .style(style);
        Self {
            settings,
        }
    }
}

impl Renderer for EguiRenderer {
    type Out = Vec<DrawCmd>;

    fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    fn prepare(&self, out: &mut Vec<DrawCmd>, width: FontUnit, height: FontUnit) {
        // We don't emit commands here, but we note the canvas size.
        // (We'll set it after render via the struct fields.)
        let _ = (out, width, height);
    }

    fn finish(&self, _out: &mut Vec<DrawCmd>) {}

    fn symbol(&self, out: &mut Vec<DrawCmd>, pos: Cursor, codepoint: u32, scale: Float) {
        let x = f64::from(pos.x) / UNITS_PER_EM * self.settings.font_size as f64;
        let y = f64::from(pos.y) / UNITS_PER_EM * self.settings.font_size as f64;
        out.push(DrawCmd::Glyph {
            x,
            y,
            codepoint,
            scale,
        });
    }

    fn rule(&self, out: &mut Vec<DrawCmd>, pos: Cursor, width: FontUnit, height: FontUnit) {
        let x = f64::from(pos.x) / UNITS_PER_EM * self.settings.font_size as f64;
        let y = f64::from(pos.y) / UNITS_PER_EM * self.settings.font_size as f64;
        let w = f64::from(width) / UNITS_PER_EM * self.settings.font_size as f64;
        let h = f64::from(height) / UNITS_PER_EM * self.settings.font_size as f64;
        out.push(DrawCmd::Rule {
            x,
            y,
            width: w,
            height: h,
        });
    }

    fn color<F>(&self, out: &mut Vec<DrawCmd>, color: RGBA, mut contents: F)
    where
        F: FnMut(&Self, &mut Vec<DrawCmd>),
    {
        out.push(DrawCmd::PushColor(color));
        contents(self, out);
        out.push(DrawCmd::PopColor);
    }

    fn render_to(&self, out: &mut Vec<DrawCmd>, tex: &str) -> Result<(), rex::error::Error> {
        // Call the default implementation from the trait
        let mut parse = rex::parser::parse(tex)?;
        let layout = rex::layout::engine::layout(&mut parse, self.settings.layout_settings());

        let padding = (self.settings.horz_padding, self.settings.vert_padding);

        let total_width = layout.width + 2 * padding.0;
        let total_height = layout.height - layout.depth + 2 * padding.1;

        // We can't mutate self here (trait signature), so we store canvas size
        // in the output as a special first command. Actually, let's just compute
        // it in the calling code. We proceed with rendering:

        let pos = Cursor {
            x: padding.0,
            y: padding.1 + layout.height,
        };
        self.render_hbox(
            out,
            pos,
            &layout.contents,
            layout.height,
            layout.width,
            rex::layout::Alignment::Default,
        );

        // Store canvas dimensions as metadata at the end (we'll read them back)
        // Encode as a special Glyph with codepoint 0
        out.push(DrawCmd::Glyph {
            x: f64::from(total_width) / UNITS_PER_EM * self.settings.font_size as f64,
            y: f64::from(total_height) / UNITS_PER_EM * self.settings.font_size as f64,
            codepoint: 0, // sentinel
            scale: 0.0,   // sentinel
        });

        Ok(())
    }
}

// ── Rendering into egui ─────────────────────────────────────────────────

/// Pre-layout math: parse and get draw commands + size without painting.
/// Returns (commands, width, height) or None on parse error.
pub fn layout_math(
    tex: &str,
    font_size: f32,
    display: bool,
) -> Option<(Vec<DrawCmd>, f32, f32)> {
    // Preprocess to handle unsupported commands
    let tex = preprocess_latex(tex);
    
    let renderer = EguiRenderer::new(font_size as u16, display);
    let mut commands = Vec::new();
    match renderer.render_to(&mut commands, &tex) {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!("ReX parse error for {:?}: {}", tex, e);
            return None;
        }
    }

    let (w, h) = extract_canvas_size(&mut commands);
    Some((commands, w, h))
}

/// Preprocess LaTeX to handle commands that ReX doesn't support.
fn preprocess_latex(tex: &str) -> String {
    let mut result = tex.to_string();
    
    // \boxed{...} -> just render the content (we lose the box, but at least it renders)
    // A proper fix would add box support to ReX
    while let Some(start) = result.find("\\boxed{") {
        let brace_start = start + 7;
        if let Some(end) = find_matching_brace(&result[brace_start..]) {
            let content = &result[brace_start..brace_start + end];
            result = format!(
                "{}{}{}",
                &result[..start],
                content,
                &result[brace_start + end + 1..]
            );
        } else {
            break;
        }
    }
    
    // \cancel{...} -> just render the content
    while let Some(start) = result.find("\\cancel{") {
        let brace_start = start + 8;
        if let Some(end) = find_matching_brace(&result[brace_start..]) {
            let content = &result[brace_start..brace_start + end];
            result = format!(
                "{}{}{}",
                &result[..start],
                content,
                &result[brace_start + end + 1..]
            );
        } else {
            break;
        }
    }
    
    // \textcolor{color}{...} -> just render the content
    while let Some(start) = result.find("\\textcolor{") {
        let first_brace = start + 11;
        // Skip the color argument
        if let Some(color_end) = find_matching_brace(&result[first_brace..]) {
            let after_color = first_brace + color_end + 1;
            // Now find the content brace
            if result[after_color..].starts_with('{') {
                if let Some(content_end) = find_matching_brace(&result[after_color + 1..]) {
                    let content = &result[after_color + 1..after_color + 1 + content_end];
                    result = format!(
                        "{}{}{}",
                        &result[..start],
                        content,
                        &result[after_color + 1 + content_end + 1..]
                    );
                    continue;
                }
            }
        }
        break;
    }
    
    // \big, \Big, \bigg, \Bigg (and variants like \biggl, \biggr, \bigm etc.)
    // ReX has these but its expect_type is strict about atom types, so \big( fails.
    // Strip the size prefix and just keep the delimiter.
    for prefix in &["\\Bigg", "\\bigg", "\\Big", "\\big"] {
        // Match \bigl, \bigr, \bigm and bare \big etc.
        // We need to be careful: \bigg must be checked before \big
        let mut new_result = String::new();
        let mut remaining = result.as_str();
        while let Some(pos) = remaining.find(prefix) {
            new_result.push_str(&remaining[..pos]);
            let after = &remaining[pos + prefix.len()..];
            // Skip optional suffix: l, r, m
            let after = if after.starts_with('l') || after.starts_with('r') || after.starts_with('m') {
                &after[1..]
            } else {
                after
            };
            remaining = after;
        }
        new_result.push_str(remaining);
        result = new_result;
    }
    
    result
}

/// Find the position of the matching closing brace, accounting for nesting.
/// Returns the index of the closing brace relative to the start of the string.
fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Paint pre-computed math draw commands at a given origin.
pub fn paint_math_commands(
    painter: &egui::Painter,
    commands: &[DrawCmd],
    origin: egui::Pos2,
    font_size: f32,
    color: egui::Color32,
) {
    let mut color_stack: Vec<egui::Color32> = vec![color];

    for cmd in commands {
        match cmd {
            DrawCmd::Glyph {
                x, y, codepoint, scale,
            } => {
                if *codepoint == 0 { continue; }
                let current_color = *color_stack.last().unwrap_or(&color);
                if let Some(ch) = char::from_u32(*codepoint) {
                    let glyph_size = font_size * (*scale as f32);
                    let pos = egui::pos2(origin.x + *x as f32, origin.y + *y as f32);
                    let baseline_offset = glyph_size * 0.8;
                    let draw_pos = egui::pos2(pos.x, pos.y - baseline_offset);
                    let math_font = egui::FontId {
                        size: glyph_size,
                        family: egui::FontFamily::Name("Math".into()),
                    };
                    let galley = painter.layout_no_wrap(
                        ch.to_string(), math_font, current_color,
                    );
                    painter.galley(draw_pos, galley, current_color);
                }
            }
            DrawCmd::Rule { x, y, width, height } => {
                let current_color = *color_stack.last().unwrap_or(&color);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(origin.x + *x as f32, origin.y + *y as f32),
                    egui::vec2(*width as f32, *height as f32),
                );
                painter.rect_filled(rect, 0.0, current_color);
            }
            DrawCmd::PushColor(rgba) => {
                let c = if rgba.has_alpha() {
                    egui::Color32::from_rgba_premultiplied(rgba.0, rgba.1, rgba.2, rgba.3)
                } else {
                    egui::Color32::from_rgb(rgba.0, rgba.1, rgba.2)
                };
                color_stack.push(c);
            }
            DrawCmd::PopColor => {
                if color_stack.len() > 1 { color_stack.pop(); }
            }
        }
    }
}

fn extract_canvas_size(commands: &mut Vec<DrawCmd>) -> (f32, f32) {
    if let Some(DrawCmd::Glyph { x, y, codepoint: 0, scale }) = commands.last() {
        if *scale == 0.0 {
            let dims = (*x as f32, *y as f32);
            commands.pop();
            return dims;
        }
    }
    (200.0, 40.0)
}

/// Render a LaTeX math string into the given egui `Ui`.
/// `display` controls whether to use Display style (large fractions, centered limits)
/// or Text/inline style (smaller fractions, side limits).
/// Returns the size of the rendered math, or None on parse error.
pub fn render_math_ui(
    ui: &mut egui::Ui,
    tex: &str,
    font_size: f32,
    color: egui::Color32,
    display: bool,
) -> Option<egui::Vec2> {
    let (commands, canvas_w, canvas_h) = layout_math(tex, font_size, display)?;

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(canvas_w, canvas_h),
        egui::Sense::hover(),
    );

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        paint_math_commands(&painter, &commands, rect.left_top(), font_size, color);
    }

    Some(egui::vec2(canvas_w, canvas_h))
}

/// Render a single line of text with inline math (no wrapping).
/// Useful for sidebar titles and other compact displays.
/// Returns the total width of the rendered content.
pub fn render_inline_math_line(
    ui: &mut egui::Ui,
    text: &str,
    font_size: f32,
    color: egui::Color32,
) -> f32 {
    // Parse the text for $...$ inline math
    let segments = parse_simple_inline_math(text);
    
    // Calculate total width and max height
    let mut total_width = 0.0f32;
    let mut max_height = font_size * 1.2;
    
    // Pre-compute sizes
    struct MeasuredPart {
        is_math: bool,
        content: String,
        width: f32,
        height: f32,
        commands: Option<Vec<DrawCmd>>,
    }
    
    let mut parts: Vec<MeasuredPart> = Vec::new();
    
    for (is_math, content) in &segments {
        if *is_math {
            if let Some((commands, w, h)) = layout_math(content, font_size, false) {
                total_width += w;
                max_height = max_height.max(h);
                parts.push(MeasuredPart {
                    is_math: true,
                    content: content.clone(),
                    width: w,
                    height: h,
                    commands: Some(commands),
                });
            } else {
                // Fallback: render as text with $ delimiters
                let fallback = format!("${}$", content);
                let galley = ui.painter().layout_no_wrap(
                    fallback.clone(),
                    egui::FontId::proportional(font_size),
                    color,
                );
                total_width += galley.size().x;
                parts.push(MeasuredPart {
                    is_math: false,
                    content: fallback,
                    width: galley.size().x,
                    height: galley.size().y,
                    commands: None,
                });
            }
        } else {
            let galley = ui.painter().layout_no_wrap(
                content.clone(),
                egui::FontId::proportional(font_size),
                color,
            );
            total_width += galley.size().x;
            parts.push(MeasuredPart {
                is_math: false,
                content: content.clone(),
                width: galley.size().x,
                height: galley.size().y,
                commands: None,
            });
        }
    }
    
    // Allocate space and render
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(total_width, max_height),
        egui::Sense::hover(),
    );
    
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let mut x = rect.left();
        let baseline_y = rect.top() + font_size;
        
        for part in &parts {
            if part.is_math {
                if let Some(ref commands) = part.commands {
                    // Center math vertically
                    let math_y = rect.top() + (max_height - part.height) / 2.0;
                    paint_math_commands(&painter, commands, egui::pos2(x, math_y), font_size, color);
                }
            } else {
                let galley = painter.layout_no_wrap(
                    part.content.clone(),
                    egui::FontId::proportional(font_size),
                    color,
                );
                let text_y = baseline_y - galley.size().y * 0.8;
                painter.galley(egui::pos2(x, text_y), galley, color);
            }
            x += part.width;
        }
    }
    
    total_width
}

/// Parse text for inline math, collapsing all delimiter types to inline.
/// Handles: $...$, $$...$$, \(...\) - all rendered as inline math.
/// Used for chat titles where we want compact single-line rendering.
fn parse_simple_inline_math(text: &str) -> Vec<(bool, String)> {
    let mut result = Vec::new();
    let mut remaining = text;
    
    while !remaining.is_empty() {
        // Find the earliest math delimiter
        let dollar_pos = remaining.find('$');
        let paren_pos = remaining.find("\\(");
        
        let (delim_type, start_pos) = match (dollar_pos, paren_pos) {
            (Some(d), Some(p)) => if d <= p { ("dollar", d) } else { ("paren", p) },
            (Some(d), None) => ("dollar", d),
            (None, Some(p)) => ("paren", p),
            (None, None) => {
                // No more math
                if !remaining.is_empty() {
                    result.push((false, remaining.to_string()));
                }
                break;
            }
        };
        
        // Add text before the delimiter
        if start_pos > 0 {
            result.push((false, remaining[..start_pos].to_string()));
        }
        
        if delim_type == "paren" {
            // \(...\) format
            remaining = &remaining[start_pos + 2..]; // skip \(
            
            if let Some(end_idx) = remaining.find("\\)") {
                let math = remaining[..end_idx].trim();
                if !math.is_empty() {
                    result.push((true, math.to_string()));
                }
                remaining = &remaining[end_idx + 2..]; // skip \)
            } else {
                // No closing \), treat as literal
                result.push((false, "\\(".to_string()));
            }
        } else {
            // $ or $$ format
            remaining = &remaining[start_pos + 1..]; // skip first $
            
            // Check for $$ (display math - but we'll render as inline for titles)
            let is_display = remaining.starts_with('$');
            if is_display {
                remaining = &remaining[1..]; // skip second $
            }
            
            if is_display {
                // For $$, find closing $$
                if let Some(end_idx) = remaining.find("$$") {
                    let math = remaining[..end_idx].trim();
                    if !math.is_empty() {
                        result.push((true, math.to_string()));
                    }
                    remaining = &remaining[end_idx + 2..];
                } else {
                    // No closing $$, treat as literal
                    result.push((false, "$$".to_string()));
                }
            } else {
                // For single $, find closing $ (not crossing newline)
                let mut end = None;
                for (i, c) in remaining.char_indices() {
                    if c == '$' {
                        end = Some(i);
                        break;
                    }
                    if c == '\n' {
                        break;
                    }
                }
                
                if let Some(end_idx) = end {
                    let math = remaining[..end_idx].trim();
                    if !math.is_empty() {
                        result.push((true, math.to_string()));
                    }
                    remaining = &remaining[end_idx + 1..];
                } else {
                    // No closing $, treat as literal
                    result.push((false, "$".to_string()));
                }
            }
        }
    }
    
    result
}

/// Render a paragraph with inline math using manual word-wrapping.
///
/// We measure each word and math expression, place them left-to-right,
/// and wrap to the next line when we run out of space. This gives true
/// inline flow without fighting egui's widget layout model.
pub fn render_inline_paragraph(
    ui: &mut egui::Ui,
    segments: &[Segment],
    text_size: f32,
    math_size: f32,
    color: egui::Color32,
) {
    let max_width = ui.available_width();
    let line_height = text_size * 1.4; // approximate line height
    let space_width = {
        let galley = ui.painter().layout_no_wrap(
            " ".to_string(),
            egui::FontId::proportional(text_size),
            color,
        );
        galley.size().x
    };

    // Break segments into "tokens" - individual words and math expressions
    enum Token {
        Word { text: String, bold: bool, italic: bool, code: bool, strikethrough: bool },
        Math { tex: String, commands: Vec<DrawCmd>, width: f32, height: f32 },
        MathFallback(String),
        Space,
    }

    let mut tokens: Vec<Token> = Vec::new();

    for seg in segments {
        match seg {
            Segment::Text(text) => {
                let text = text.replace('\n', " ");
                for (i, word) in text.split(' ').enumerate() {
                    if i > 0 {
                        tokens.push(Token::Space);
                    }
                    if !word.is_empty() {
                        tokens.push(Token::Word {
                            text: word.to_string(),
                            bold: false,
                            italic: false,
                            code: false,
                            strikethrough: false,
                        });
                    }
                }
            }
            Segment::StyledText { text, bold, italic, code, strikethrough } => {
                let text = text.replace('\n', " ");
                for (i, word) in text.split(' ').enumerate() {
                    if i > 0 {
                        tokens.push(Token::Space);
                    }
                    if !word.is_empty() {
                        tokens.push(Token::Word {
                            text: word.to_string(),
                            bold: *bold,
                            italic: *italic,
                            code: *code,
                            strikethrough: *strikethrough,
                        });
                    }
                }
            }
            Segment::InlineMath(tex) => {
                if let Some((commands, w, h)) = layout_math(tex, math_size, false) {
                    tokens.push(Token::Math {
                        tex: tex.clone(),
                        commands,
                        width: w,
                        height: h,
                    });
                } else {
                    tokens.push(Token::MathFallback(format!("${}$", tex)));
                }
            }
            _ => {}
        }
    }

    // Measure each token's width
    let mut measured: Vec<f32> = Vec::with_capacity(tokens.len());
    for token in &tokens {
        let w = match token {
            Token::Word { text, code, .. } => {
                let font = if *code {
                    egui::FontId::monospace(text_size)
                } else {
                    egui::FontId::proportional(text_size)
                };
                let galley = ui.painter().layout_no_wrap(
                    text.clone(),
                    font,
                    color,
                );
                galley.size().x
            }
            Token::Math { width, .. } => *width,
            Token::MathFallback(text) => {
                let galley = ui.painter().layout_no_wrap(
                    text.clone(),
                    egui::FontId::proportional(text_size),
                    color,
                );
                galley.size().x
            }
            Token::Space => space_width,
        };
        measured.push(w);
    }

    // Line-break: greedily place tokens, wrapping when they exceed max_width
    struct Line {
        tokens: Vec<usize>, // indices into tokens vec
        width: f32,
    }

    let mut lines: Vec<Line> = vec![Line { tokens: Vec::new(), width: 0.0 }];

    for (i, token) in tokens.iter().enumerate() {
        let w = measured[i];
        let current_line = lines.last_mut().unwrap();

        // Spaces at start of line are skipped
        if matches!(token, Token::Space) && current_line.tokens.is_empty() {
            continue;
        }

        if current_line.width + w > max_width && !current_line.tokens.is_empty() {
            // Wrap to next line (skip leading space)
            if matches!(token, Token::Space) {
                continue;
            }
            lines.push(Line { tokens: vec![i], width: w });
        } else {
            current_line.tokens.push(i);
            current_line.width += w;
        }
    }

    // Calculate total height
    let total_height = lines.len() as f32 * line_height;

    // Allocate space
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(max_width, total_height),
        egui::Sense::hover(),
    );

    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter_at(rect);

    // Paint each line
    for (line_idx, line) in lines.iter().enumerate() {
        let y = rect.top() + line_idx as f32 * line_height;
        let baseline_y = y + text_size; // approximate baseline position
        let mut x = rect.left();

        for &token_idx in &line.tokens {
            let token = &tokens[token_idx];
            match token {
                Token::Word { text, bold, italic, code, strikethrough } => {
                    // Build the appropriate font
                    let font = if *code {
                        egui::FontId::monospace(text_size)
                    } else {
                        egui::FontId::proportional(text_size)
                    };
                    
                    // Create a LayoutJob for styled text
                    let mut job = egui::text::LayoutJob::default();
                    let mut text_format = egui::TextFormat {
                        font_id: font,
                        color,
                        ..Default::default()
                    };
                    
                    // Apply strikethrough
                    if *strikethrough {
                        text_format.strikethrough = egui::Stroke::new(1.0, color);
                    }
                    
                    // Apply code background
                    if *code {
                        text_format.background = color.gamma_multiply(0.1);
                    }
                    
                    job.append(text, 0.0, text_format);
                    
                    let galley = painter.layout_job(job);
                    let text_y = baseline_y - galley.size().y * 0.8;
                    
                    // For bold/italic we need to simulate since egui doesn't have real bold/italic
                    // We'll draw the text, and for bold draw it again slightly offset
                    painter.galley(egui::pos2(x, text_y), galley.clone(), color);
                    
                    if *bold {
                        // Fake bold by drawing again with slight offset
                        let mut bold_job = egui::text::LayoutJob::default();
                        let font = if *code {
                            egui::FontId::monospace(text_size)
                        } else {
                            egui::FontId::proportional(text_size)
                        };
                        let text_format = egui::TextFormat {
                            font_id: font,
                            color,
                            ..Default::default()
                        };
                        bold_job.append(text, 0.0, text_format);
                        let bold_galley = painter.layout_job(bold_job);
                        painter.galley(egui::pos2(x + 0.5, text_y), bold_galley, color);
                    }
                    
                    if *italic {
                        // For italic, we can't easily do a transform, so we'll rely on the 
                        // text having visual distinction in other ways, or just note it
                        // egui doesn't support text shear transforms easily
                    }
                    
                    x += galley.size().x;
                }
                Token::Math { commands, width, height, .. } => {
                    // Center math vertically on the line
                    let math_y = y + (line_height - height) / 2.0;
                    paint_math_commands(&painter, commands, egui::pos2(x, math_y), math_size, color);
                    x += width;
                }
                Token::MathFallback(text) => {
                    let galley = painter.layout_no_wrap(
                        text.clone(),
                        egui::FontId::proportional(text_size),
                        egui::Color32::from_rgb(150, 150, 150),
                    );
                    let text_y = baseline_y - galley.size().y * 0.8;
                    painter.galley(egui::pos2(x, text_y), galley.clone(), color);
                    x += galley.size().x;
                }
                Token::Space => {
                    x += space_width;
                }
            }
        }
    }
}

// ── Math segment parsing ────────────────────────────────────────────────

/// A segment of text that is either plain markdown or a LaTeX math block.
#[derive(Debug, Clone)]
pub enum Segment {
    /// Regular markdown text.
    Text(String),
    /// Styled text with formatting flags.
    StyledText {
        text: String,
        bold: bool,
        italic: bool,
        code: bool,
        strikethrough: bool,
    },
    /// Inline math (was delimited by `$...$`).
    InlineMath(String),
    /// Display math (was delimited by `$$...$$`).
    DisplayMath(String),
}

/// Parse a string into segments of text and math.
/// Recognizes `$$...$$` (display math) and `$...$` (inline math).
pub fn parse_math_segments(input: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut chars = input.char_indices().peekable();
    let mut text_start = 0;

    while let Some(&(i, ch)) = chars.peek() {
        if ch == '$' {
            // Check for $$ (display math)
            let next = {
                let mut lookahead = chars.clone();
                lookahead.next();
                lookahead.peek().map(|&(_, c)| c)
            };

            if next == Some('$') {
                // Display math $$...$$
                // Flush preceding text
                if i > text_start {
                    segments.push(Segment::Text(input[text_start..i].to_string()));
                }

                // Skip the opening $$
                chars.next();
                chars.next();

                // Find closing $$
                let math_start = i + 2;
                let mut math_end = None;
                while let Some(&(j, c)) = chars.peek() {
                    if c == '$' {
                        let next2 = {
                            let mut la = chars.clone();
                            la.next();
                            la.peek().map(|&(_, c2)| c2)
                        };
                        if next2 == Some('$') {
                            math_end = Some(j);
                            chars.next();
                            chars.next();
                            break;
                        }
                    }
                    chars.next();
                }

                if let Some(end) = math_end {
                    let math_content = input[math_start..end].trim();
                    if !math_content.is_empty() {
                        segments.push(Segment::DisplayMath(math_content.to_string()));
                    }
                    text_start = end + 2;
                } else {
                    // No closing $$ found, treat as text
                    text_start = i;
                    // chars already consumed
                }
            } else {
                // Inline math $...$
                // But skip if preceded by backslash (escaped)
                let escaped = i > 0 && input.as_bytes()[i - 1] == b'\\';
                if escaped {
                    chars.next();
                    continue;
                }

                // Flush preceding text
                if i > text_start {
                    segments.push(Segment::Text(input[text_start..i].to_string()));
                }

                // Skip the opening $
                chars.next();

                // Find closing $
                let math_start = i + 1;
                let mut math_end = None;
                while let Some(&(j, c)) = chars.peek() {
                    if c == '$' {
                        // Don't match $$ as end of inline math
                        math_end = Some(j);
                        chars.next();
                        break;
                    }
                    if c == '\n' {
                        // Inline math doesn't span lines
                        break;
                    }
                    chars.next();
                }

                if let Some(end) = math_end {
                    let math_content = input[math_start..end].trim();
                    if !math_content.is_empty() {
                        segments.push(Segment::InlineMath(math_content.to_string()));
                    }
                    text_start = end + 1;
                } else {
                    // No closing $ found, treat as text
                    segments.push(Segment::Text(input[text_start..math_start].to_string()));
                    text_start = math_start;
                }
            }
        } else {
            chars.next();
        }
    }

    // Flush remaining text
    if text_start < input.len() {
        segments.push(Segment::Text(input[text_start..].to_string()));
    }

    segments
}

/// Returns true if the content contains any math delimiters.
/// Checks for: $, \(, \)
pub fn contains_math(content: &str) -> bool {
    content.contains('$') || content.contains("\\(") || content.contains("\\)")
}

// ── Block-level splitting ───────────────────────────────────────────────

/// A block-level element in the rendered output.
#[derive(Debug, Clone)]
pub enum Block {
    /// Display math ($$...$$), rendered centered on its own line.
    DisplayMath(String),
    /// A paragraph that contains inline math mixed with text.
    /// Rendered in a single horizontal flow.
    InlineMathParagraph(Vec<Segment>),
    /// A heading that contains inline math.
    /// `level` is 1-6 (from # to ######).
    HeadingWithMath { level: u8, segments: Vec<Segment> },
    /// A list item that contains inline math.
    ListItemWithMath { segments: Vec<Segment> },
    /// Plain markdown text with no inline math.
    /// Rendered via CommonMarkViewer for full markdown support.
    Markdown(String),
}

/// Returns true if text contains markdown block-level syntax that
/// CommonMarkViewer should handle (headings, tables, HRs, lists, code fences).
fn has_block_markdown(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("***")
            || trimmed.starts_with("___")
            || trimmed.starts_with('|')
            || trimmed.starts_with("```")
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("> ")
            || (trimmed.len() > 2
                && trimmed.as_bytes()[0].is_ascii_digit()
                && trimmed.contains(". "))
    })
}

/// Split message content into blocks for rendering.
///
/// The strategy:
/// 1. Parse into segments (Text, InlineMath, DisplayMath)
/// 2. Group consecutive Text+InlineMath segments
/// 3. Within each group, split on \n\n to form paragraphs
/// 4. For each paragraph, if it has inline math AND no block-level markdown,
///    emit InlineMathParagraph; otherwise emit Markdown
/// 5. DisplayMath segments become their own blocks
pub fn split_into_blocks(content: &str) -> Vec<Block> {
    let segments = parse_math_segments(content);
    let mut blocks = Vec::new();

    // Walk segments, grouping Text+InlineMath runs
    let mut i = 0;
    while i < segments.len() {
        match &segments[i] {
            Segment::DisplayMath(tex) => {
                blocks.push(Block::DisplayMath(tex.clone()));
                i += 1;
            }
            _ => {
                // Collect a run of Text + InlineMath
                let run_start = i;
                while i < segments.len()
                    && !matches!(&segments[i], Segment::DisplayMath(_))
                {
                    i += 1;
                }
                let run = &segments[run_start..i];

                // Now split this run into paragraphs.
                // Split on \n\n (paragraph breaks) and also on \n when the next
                // line starts with a list marker (- or * or digit.) to preserve
                // list structure.
                let mut current_para: Vec<Segment> = Vec::new();

                for seg in run {
                    match seg {
                        Segment::InlineMath(tex) => {
                            current_para.push(Segment::InlineMath(tex.clone()));
                        }
                        Segment::Text(text) => {
                            // Split intelligently on line boundaries
                            let mut remaining = text.as_str();
                            while !remaining.is_empty() {
                                // Find next newline
                                if let Some(nl_pos) = remaining.find('\n') {
                                    let before = &remaining[..nl_pos];
                                    let after = &remaining[nl_pos + 1..];
                                    
                                    // Check if this is a paragraph break (\n\n) or list item boundary
                                    let is_para_break = after.starts_with('\n');
                                    let next_line_is_list = {
                                        let trimmed = after.trim_start();
                                        trimmed.starts_with("- ") 
                                            || trimmed.starts_with("* ")
                                            || (trimmed.len() > 2 
                                                && trimmed.as_bytes().first().map(|b| b.is_ascii_digit()).unwrap_or(false)
                                                && trimmed.contains(". "))
                                    };
                                    let current_is_list = {
                                        let trimmed = before.trim_start();
                                        trimmed.starts_with("- ") 
                                            || trimmed.starts_with("* ")
                                            || (trimmed.len() > 2 
                                                && trimmed.as_bytes().first().map(|b| b.is_ascii_digit()).unwrap_or(false)
                                                && trimmed.contains(". "))
                                    };
                                    
                                    if !before.is_empty() {
                                        current_para.push(Segment::Text(before.to_string()));
                                    }
                                    
                                    if is_para_break || next_line_is_list || current_is_list {
                                        flush_paragraph(&mut blocks, &mut current_para);
                                        // Skip extra newline for \n\n case
                                        remaining = after.trim_start_matches('\n');
                                    } else {
                                        // Single newline within a paragraph - keep as space
                                        if !current_para.is_empty() {
                                            current_para.push(Segment::Text(" ".to_string()));
                                        }
                                        remaining = after;
                                    }
                                } else {
                                    // No more newlines
                                    if !remaining.is_empty() {
                                        current_para.push(Segment::Text(remaining.to_string()));
                                    }
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                flush_paragraph(&mut blocks, &mut current_para);
            }
        }
    }

    blocks
}

fn flush_paragraph(blocks: &mut Vec<Block>, para: &mut Vec<Segment>) {
    if para.is_empty() {
        return;
    }

    let has_math = para.iter().any(|s| matches!(s, Segment::InlineMath(_)));

    if !has_math {
        // Pure text - reassemble and emit as Markdown
        let mut text = String::new();
        for seg in para.iter() {
            if let Segment::Text(t) = seg {
                text.push_str(t);
            }
        }
        if !text.trim().is_empty() {
            blocks.push(Block::Markdown(text));
        }
    } else {
        // Has inline math - check if the text parts have block-level markdown
        let first_text = para
            .iter()
            .find_map(|s| if let Segment::Text(t) = s { Some(t.as_str()) } else { None })
            .unwrap_or("");
        let trimmed = first_text.trim_start();

        // Check if this is a heading with math
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count().min(6) as u8;
            // Strip the heading prefix from the first text segment
            let mut segments: Vec<Segment> = para.drain(..).collect();
            if let Some(Segment::Text(ref mut t)) = segments.first_mut() {
                let hash_end = t.find(|c: char| c != '#').unwrap_or(t.len());
                *t = t[hash_end..].trim_start().to_string();
            }
            blocks.push(Block::HeadingWithMath { level, segments });
            return;
        }

        // Check if this is a list item with math (- or * or 1. prefix)
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") ||
           (trimmed.len() > 2 && trimmed.as_bytes()[0].is_ascii_digit() && trimmed.contains(". "))
        {
            let mut segments: Vec<Segment> = para.drain(..).collect();
            if let Some(Segment::Text(ref mut t)) = segments.first_mut() {
                // Strip the list marker
                let trimmed_t = t.trim_start();
                if trimmed_t.starts_with("- ") || trimmed_t.starts_with("* ") {
                    let marker_end = t.find("- ").or_else(|| t.find("* ")).unwrap_or(0) + 2;
                    *t = t[marker_end..].to_string();
                } else if let Some(dot_pos) = trimmed_t.find(". ") {
                    let start = t.len() - trimmed_t.len();
                    *t = t[start + dot_pos + 2..].to_string();
                }
            }
            blocks.push(Block::ListItemWithMath { segments });
            return;
        }

        let text_parts: String = para
            .iter()
            .filter_map(|s| {
                if let Segment::Text(t) = s {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect();

        if has_block_markdown(&text_parts) {
            // Block markdown present (tables, HRs, etc.) - can't do inline flow.
            // Render each piece separately.
            for seg in para.drain(..) {
                match seg {
                    Segment::Text(t) if !t.trim().is_empty() => {
                        blocks.push(Block::Markdown(t));
                    }
                    Segment::InlineMath(tex) => {
                        blocks.push(Block::InlineMathParagraph(vec![
                            Segment::InlineMath(tex),
                        ]));
                    }
                    _ => {}
                }
            }
            return;
        }

        // Simple paragraph with inline math - render as flow
        blocks.push(Block::InlineMathParagraph(para.drain(..).collect()));
    }

    para.clear();
}
