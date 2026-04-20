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
    let renderer = EguiRenderer::new(font_size as u16, display);
    let mut commands = Vec::new();
    match renderer.render_to(&mut commands, tex) {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!("ReX parse error for {:?}: {}", tex, e);
            return None;
        }
    }

    // Extract canvas size from sentinel at end
    let (canvas_w, canvas_h) = if let Some(DrawCmd::Glyph {
        x,
        y,
        codepoint: 0,
        scale,
    }) = commands.last()
    {
        if *scale == 0.0 {
            let dims = (*x as f32, *y as f32);
            commands.pop();
            dims
        } else {
            (200.0, 40.0) // fallback
        }
    } else {
        (200.0, 40.0)
    };

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(canvas_w, canvas_h),
        egui::Sense::hover(),
    );

    if !ui.is_rect_visible(rect) {
        return Some(egui::vec2(canvas_w, canvas_h));
    }

    let painter = ui.painter_at(rect);
    let origin = rect.left_top();

    let mut color_stack: Vec<egui::Color32> = vec![color];

    for cmd in &commands {
        match cmd {
            DrawCmd::Glyph {
                x,
                y,
                codepoint,
                scale,
            } => {
                if *codepoint == 0 {
                    continue;
                }
                let current_color = *color_stack.last().unwrap_or(&color);
                if let Some(ch) = char::from_u32(*codepoint) {
                    let glyph_size = font_size * (*scale as f32);
                    let pos = egui::pos2(origin.x + *x as f32, origin.y + *y as f32);

                    // egui draws text from top-left, but ReX positions
                    // are at the baseline. We need to shift up by the ascent.
                    // XITS Math has ascent ~0.8 of em.
                    let baseline_offset = glyph_size * 0.8;
                    let draw_pos = egui::pos2(pos.x, pos.y - baseline_offset);

                    // Use the "Math" font family (XITS Math) for correct glyph rendering
                    let math_font = egui::FontId {
                        size: glyph_size,
                        family: egui::FontFamily::Name("Math".into()),
                    };
                    let galley = ui.painter().layout_no_wrap(
                        ch.to_string(),
                        math_font,
                        current_color,
                    );

                    painter.galley(draw_pos, galley, current_color);
                }
            }
            DrawCmd::Rule {
                x,
                y,
                width,
                height,
            } => {
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
                if color_stack.len() > 1 {
                    color_stack.pop();
                }
            }
        }
    }

    Some(egui::vec2(canvas_w, canvas_h))
}

// ── Math segment parsing ────────────────────────────────────────────────

/// A segment of text that is either plain markdown or a LaTeX math block.
#[derive(Debug, Clone)]
pub enum Segment {
    /// Regular markdown text.
    Text(String),
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
pub fn contains_math(content: &str) -> bool {
    content.contains('$')
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

                // Now split this run into paragraphs on \n\n boundaries.
                // We need to break Text segments at \n\n.
                let mut current_para: Vec<Segment> = Vec::new();

                for seg in run {
                    match seg {
                        Segment::InlineMath(tex) => {
                            current_para.push(Segment::InlineMath(tex.clone()));
                        }
                        Segment::Text(text) => {
                            // Split on \n\n
                            let parts: Vec<&str> = text.split("\n\n").collect();
                            for (pi, part) in parts.iter().enumerate() {
                                if pi > 0 {
                                    // Paragraph break - flush current_para
                                    flush_paragraph(&mut blocks, &mut current_para);
                                }
                                if !part.is_empty() {
                                    current_para.push(Segment::Text(part.to_string()));
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
            // Block markdown present - can't do inline flow.
            // Render each piece separately: text as markdown, math inline.
            // This is a compromise - the math won't flow inline with
            // markdown block elements, but at least headings/tables render.
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
