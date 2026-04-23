//! Unified Markdown + Math renderer using pulldown-cmark.
//!
//! Parses markdown with pulldown-cmark, walks the event stream, and renders
//! each element using egui primitives. Math ($...$, $$...$$) is detected in
//! text nodes and rendered via the math_render module.

use crate::math_render;
use eframe::egui;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

/// Render markdown content with math support into the given Ui.
pub fn render_markdown(
    ui: &mut egui::Ui,
    content: &str,
    base_font_size: f32,
    text_color: egui::Color32,
) {
    let mut renderer = MarkdownRenderer::new(ui, base_font_size, text_color);
    renderer.render(content);
}

struct MarkdownRenderer<'a> {
    ui: &'a mut egui::Ui,
    base_font_size: f32,
    text_color: egui::Color32,

    // Style stack
    bold: bool,
    italic: bool,
    code: bool,
    strikethrough: bool,

    // Current heading level (0 = not in heading)
    heading_level: u8,

    // List state
    list_depth: usize,
    ordered_list_index: Option<u64>,

    // Blockquote depth
    blockquote_depth: usize,

    // Code block state
    in_code_block: bool,
    code_block_content: String,
    code_block_lang: Option<String>,

    // Table state
    in_table: bool,
    in_table_head: bool,
    table_rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,

    // Accumulated text for the current paragraph/inline context
    // We accumulate text and render it all at once to handle math properly
    pending_text: Vec<TextSpan>,
}

#[derive(Clone)]
struct TextSpan {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    strikethrough: bool,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(ui: &'a mut egui::Ui, base_font_size: f32, text_color: egui::Color32) -> Self {
        Self {
            ui,
            base_font_size,
            text_color,
            bold: false,
            italic: false,
            code: false,
            strikethrough: false,
            heading_level: 0,
            list_depth: 0,
            ordered_list_index: None,
            blockquote_depth: 0,
            in_code_block: false,
            code_block_content: String::new(),
            code_block_lang: None,
            in_table: false,
            in_table_head: false,
            table_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            pending_text: Vec::new(),
        }
    }

    fn render(&mut self, content: &str) {
        // Pre-process: protect math from markdown parsing
        // Replace $...$ with placeholders, parse, then restore
        let (processed, math_blocks) = protect_math(content);

        let parser = Parser::new(&processed);

        for event in parser {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => {
                    // Restore math placeholders and handle
                    let restored = restore_math(&text, &math_blocks);
                    self.handle_text(&restored);
                }
                Event::Code(code) => self.handle_inline_code(&code),
                Event::SoftBreak => self.handle_soft_break(),
                Event::HardBreak => self.handle_hard_break(),
                Event::Rule => self.handle_rule(),
                _ => {}
            }
        }

        // Flush any remaining text
        self.flush_text();
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                // Flush before starting new paragraph
                self.flush_text();
            }
            Tag::Heading { level, .. } => {
                self.flush_text();
                self.heading_level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
            }
            Tag::BlockQuote => {
                self.flush_text();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush_text();
                self.in_code_block = true;
                self.code_block_content.clear();
                self.code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                    _ => None,
                };
            }
            Tag::List(start) => {
                self.flush_text();
                self.list_depth += 1;
                self.ordered_list_index = start;
            }
            Tag::Item => {
                self.flush_text();
            }
            Tag::Emphasis => {
                self.italic = true;
            }
            Tag::Strong => {
                self.bold = true;
            }
            Tag::Strikethrough => {
                self.strikethrough = true;
            }
            Tag::Link { .. } | Tag::Image { .. } => {
                // We'll handle the text inside, links just underline
            }
            Tag::Table(_alignments) => {
                self.flush_text();
                self.in_table = true;
                self.table_rows.clear();
            }
            Tag::TableHead => {
                self.in_table_head = true;
                self.current_row.clear();
            }
            Tag::TableRow => {
                self.current_row.clear();
            }
            Tag::TableCell => {
                self.current_cell.clear();
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_text();
                self.ui.add_space(4.0);
            }
            TagEnd::Heading(_) => {
                self.flush_text();
                // Add underline for h1/h2
                if self.heading_level <= 2 {
                    let rect = self.ui.available_rect_before_wrap();
                    self.ui.painter().line_segment(
                        [
                            egui::pos2(rect.left(), rect.top()),
                            egui::pos2(rect.right(), rect.top()),
                        ],
                        egui::Stroke::new(1.0, self.text_color.gamma_multiply(0.3)),
                    );
                    self.ui.add_space(4.0);
                }
                self.heading_level = 0;
                self.ui.add_space(4.0);
            }
            TagEnd::BlockQuote => {
                self.flush_text();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.render_code_block();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.flush_text();
                self.list_depth = self.list_depth.saturating_sub(1);
                self.ordered_list_index = None;
            }
            TagEnd::Item => {
                self.flush_text();
                // Increment ordered list counter
                if let Some(ref mut idx) = self.ordered_list_index {
                    *idx += 1;
                }
            }
            TagEnd::Emphasis => {
                self.italic = false;
            }
            TagEnd::Strong => {
                self.bold = false;
            }
            TagEnd::Strikethrough => {
                self.strikethrough = false;
            }
            TagEnd::Table => {
                self.render_table();
                self.in_table = false;
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
                if !self.current_row.is_empty() {
                    self.table_rows.push(std::mem::take(&mut self.current_row));
                }
            }
            TagEnd::TableRow => {
                if !self.current_row.is_empty() {
                    self.table_rows.push(std::mem::take(&mut self.current_row));
                }
            }
            TagEnd::TableCell => {
                self.current_row
                    .push(std::mem::take(&mut self.current_cell));
            }
            _ => {}
        }
    }

    fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_block_content.push_str(text);
            return;
        }

        // If we're in a table cell, accumulate text there
        if self.in_table {
            self.current_cell.push_str(text);
            return;
        }

        self.pending_text.push(TextSpan {
            text: text.to_string(),
            bold: self.bold,
            italic: self.italic,
            code: self.code,
            strikethrough: self.strikethrough,
        });
    }

    fn handle_inline_code(&mut self, code: &str) {
        self.pending_text.push(TextSpan {
            text: code.to_string(),
            bold: self.bold,
            italic: self.italic,
            code: true,
            strikethrough: self.strikethrough,
        });
    }

    fn handle_soft_break(&mut self) {
        // Soft break = space in markdown
        self.pending_text.push(TextSpan {
            text: " ".to_string(),
            bold: self.bold,
            italic: self.italic,
            code: self.code,
            strikethrough: self.strikethrough,
        });
    }

    fn handle_hard_break(&mut self) {
        self.flush_text();
    }

    fn handle_rule(&mut self) {
        self.flush_text();
        self.ui.add_space(4.0);
        self.ui.separator();
        self.ui.add_space(4.0);
    }

    fn render_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_block_content);

        egui::Frame::new()
            .fill(self.text_color.gamma_multiply(0.1))
            .inner_margin(8.0)
            .corner_radius(4.0)
            .show(self.ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&code)
                            .family(egui::FontFamily::Monospace)
                            .size(self.base_font_size * 0.9)
                            .color(self.text_color),
                    )
                    .wrap(),
                );
            });

        self.ui.add_space(4.0);
    }

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        let rows = std::mem::take(&mut self.table_rows);
        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

        if num_cols == 0 {
            return;
        }

        let text_color = self.text_color;
        let font_size = self.base_font_size;
        let border_color = text_color.gamma_multiply(0.3);
        let header_bg = text_color.gamma_multiply(0.1);

        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, border_color))
            .corner_radius(4.0)
            .show(self.ui, |ui| {
                egui::Grid::new("md_table")
                    .num_columns(num_cols)
                    .spacing([8.0, 4.0])
                    .min_col_width(40.0)
                    .show(ui, |ui| {
                        for (row_idx, row) in rows.iter().enumerate() {
                            let is_header = row_idx == 0;

                            for (col_idx, cell) in row.iter().enumerate() {
                                let cell_text = cell.trim();

                                if is_header {
                                    // Header row - bold with background
                                    egui::Frame::new()
                                        .fill(header_bg)
                                        .inner_margin(egui::Margin::symmetric(6, 4))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(cell_text)
                                                    .size(font_size)
                                                    .color(text_color)
                                                    .strong(),
                                            );
                                        });
                                } else {
                                    // Regular cell
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(cell_text)
                                                .size(font_size)
                                                .color(text_color),
                                        )
                                        .wrap(),
                                    );
                                }

                                // Fill remaining columns if row is short
                                if col_idx == row.len() - 1 {
                                    for _ in row.len()..num_cols {
                                        ui.label("");
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            });

        self.ui.add_space(4.0);
    }

    fn flush_text(&mut self) {
        if self.pending_text.is_empty() {
            return;
        }

        let spans = std::mem::take(&mut self.pending_text);

        // Determine font size based on context
        let font_size = if self.heading_level > 0 {
            match self.heading_level {
                1 => self.base_font_size * 1.7,
                2 => self.base_font_size * 1.4,
                3 => self.base_font_size * 1.2,
                _ => self.base_font_size * 1.1,
            }
        } else {
            self.base_font_size
        };

        // Render list bullet/number if applicable
        if self.list_depth > 0 {
            let indent = (self.list_depth - 1) as f32 * 16.0 + 8.0;
            self.ui.add_space(indent);

            let marker = if let Some(idx) = self.ordered_list_index {
                format!("{}.", idx)
            } else {
                "•".to_string()
            };

            self.ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(marker)
                        .size(font_size)
                        .color(self.text_color),
                );
                ui.add_space(4.0);

                // Render the content in a vertical container
                ui.vertical(|ui| {
                    render_spans_with_math(ui, &spans, font_size, self.text_color);
                });
            });
            return;
        }

        // Render blockquote if applicable
        if self.blockquote_depth > 0 {
            let indent = self.blockquote_depth as f32 * 8.0;
            self.ui.horizontal(|ui| {
                ui.add_space(indent);
                // Vertical bar
                let rect = ui.available_rect_before_wrap();
                ui.painter().vline(
                    rect.left(),
                    rect.top()..=rect.bottom(),
                    egui::Stroke::new(2.0, self.text_color.gamma_multiply(0.3)),
                );
                ui.add_space(8.0);

                ui.vertical(|ui| {
                    render_spans_with_math(
                        ui,
                        &spans,
                        font_size,
                        self.text_color.gamma_multiply(0.8),
                    );
                });
            });
            return;
        }

        // Regular paragraph
        render_spans_with_math(self.ui, &spans, font_size, self.text_color);
    }
}

/// Render a sequence of text spans, handling inline math.
fn render_spans_with_math(
    ui: &mut egui::Ui,
    spans: &[TextSpan],
    font_size: f32,
    text_color: egui::Color32,
) {
    // Collect all text with styles, then split into segments (text vs math)
    #[derive(Clone)]
    enum Segment {
        StyledText {
            text: String,
            bold: bool,
            italic: bool,
            code: bool,
            strikethrough: bool,
        },
        InlineMath(String),
        DisplayMath(String),
    }

    let mut segments: Vec<Segment> = Vec::new();

    for span in spans {
        // Check for math in this span
        let text = &span.text;
        let mut remaining = text.as_str();

        while !remaining.is_empty() {
            // Look for $$ first (display math)
            if let Some(start) = remaining.find("$$") {
                // Add text before
                if start > 0 {
                    segments.push(Segment::StyledText {
                        text: remaining[..start].to_string(),
                        bold: span.bold,
                        italic: span.italic,
                        code: span.code,
                        strikethrough: span.strikethrough,
                    });
                }
                remaining = &remaining[start + 2..];

                // Find closing $$
                if let Some(end) = remaining.find("$$") {
                    segments.push(Segment::DisplayMath(remaining[..end].to_string()));
                    remaining = &remaining[end + 2..];
                } else {
                    // No closing, treat as text
                    segments.push(Segment::StyledText {
                        text: "$$".to_string(),
                        bold: span.bold,
                        italic: span.italic,
                        code: span.code,
                        strikethrough: span.strikethrough,
                    });
                }
            }
            // Look for single $ (inline math)
            else if let Some(start) = remaining.find('$') {
                // Add text before
                if start > 0 {
                    segments.push(Segment::StyledText {
                        text: remaining[..start].to_string(),
                        bold: span.bold,
                        italic: span.italic,
                        code: span.code,
                        strikethrough: span.strikethrough,
                    });
                }
                remaining = &remaining[start + 1..];

                // Find closing $ (not $$, and not crossing newline)
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
                        segments.push(Segment::InlineMath(math.to_string()));
                    }
                    remaining = &remaining[end_idx + 1..];
                } else {
                    // No closing, treat as text
                    segments.push(Segment::StyledText {
                        text: "$".to_string(),
                        bold: span.bold,
                        italic: span.italic,
                        code: span.code,
                        strikethrough: span.strikethrough,
                    });
                }
            }
            // No math in remaining text
            else {
                if !remaining.is_empty() {
                    segments.push(Segment::StyledText {
                        text: remaining.to_string(),
                        bold: span.bold,
                        italic: span.italic,
                        code: span.code,
                        strikethrough: span.strikethrough,
                    });
                }
                break;
            }
        }
    }

    // Check if we have any math - if not, just render text simply
    let has_math = segments
        .iter()
        .any(|s| matches!(s, Segment::InlineMath(_) | Segment::DisplayMath(_)));

    if !has_math {
        // Simple case: no math, just render styled text
        ui.horizontal_wrapped(|ui| {
            for seg in &segments {
                if let Segment::StyledText {
                    text,
                    bold,
                    italic,
                    code,
                    strikethrough,
                } = seg
                {
                    let mut rt = egui::RichText::new(text).size(font_size).color(text_color);
                    if *bold {
                        rt = rt.strong();
                    }
                    if *italic {
                        rt = rt.italics();
                    }
                    if *code {
                        rt = rt
                            .family(egui::FontFamily::Monospace)
                            .background_color(text_color.gamma_multiply(0.1));
                    }
                    if *strikethrough {
                        rt = rt.strikethrough();
                    }
                    ui.label(rt);
                }
            }
        });
        return;
    }

    // Has math - need to use our manual word-wrap renderer
    // Convert segments to math_render::Segment format
    let mut math_segments: Vec<math_render::Segment> = Vec::new();

    for seg in &segments {
        match seg {
            Segment::StyledText {
                text,
                bold,
                italic,
                code,
                strikethrough,
            } => {
                // Use the new StyledText segment variant to preserve formatting
                math_segments.push(math_render::Segment::StyledText {
                    text: text.clone(),
                    bold: *bold,
                    italic: *italic,
                    code: *code,
                    strikethrough: *strikethrough,
                });
            }
            Segment::InlineMath(tex) => {
                math_segments.push(math_render::Segment::InlineMath(tex.clone()));
            }
            Segment::DisplayMath(tex) => {
                // Flush inline content first
                if !math_segments.is_empty() {
                    let segs = std::mem::take(&mut math_segments);
                    math_render::render_inline_paragraph(
                        ui,
                        &segs,
                        font_size,
                        font_size * 1.1,
                        text_color,
                    );
                }
                // Render display math
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if math_render::render_math_ui(ui, tex, font_size * 1.3, text_color, true)
                        .is_none()
                    {
                        ui.label(
                            egui::RichText::new(format!("$${}$$", tex))
                                .size(font_size)
                                .color(text_color.gamma_multiply(0.6)),
                        );
                    }
                });
                ui.add_space(4.0);
            }
        }
    }

    // Flush remaining inline content
    if !math_segments.is_empty() {
        math_render::render_inline_paragraph(
            ui,
            &math_segments,
            font_size,
            font_size * 1.1,
            text_color,
        );
    }
}

/// Protect math from markdown parsing by replacing with placeholders.
/// Returns (processed_text, math_blocks) where math_blocks maps placeholder -> original.
/// Handles: $$...$$, $...$, \(...\)
fn protect_math(content: &str) -> (String, Vec<(String, String)>) {
    let mut result = String::new();
    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut remaining = content;
    let mut counter = 0;

    while !remaining.is_empty() {
        // Find the earliest math delimiter
        let double_dollar = remaining.find("$$");
        let single_dollar = remaining.find('$').filter(|&pos| {
            // Make sure it's not the start of $$
            double_dollar != Some(pos)
        });
        let paren_open = remaining.find("\\(");

        // Find which comes first
        let mut earliest: Option<(&str, usize)> = None;

        if let Some(pos) = double_dollar {
            earliest = Some(("$$", pos));
        }
        if let Some(pos) = single_dollar {
            if earliest.is_none() || pos < earliest.unwrap().1 {
                earliest = Some(("$", pos));
            }
        }
        if let Some(pos) = paren_open {
            if earliest.is_none() || pos < earliest.unwrap().1 {
                earliest = Some(("\\(", pos));
            }
        }

        match earliest {
            None => {
                // No more math
                result.push_str(remaining);
                break;
            }
            Some(("$$", start)) => {
                result.push_str(&remaining[..start]);
                remaining = &remaining[start + 2..];

                if let Some(end) = remaining.find("$$") {
                    let math = &remaining[..end];
                    let placeholder = format!("\u{FFFC}MATH{}D\u{FFFC}", counter);
                    blocks.push((placeholder.clone(), format!("$${}$$", math)));
                    result.push_str(&placeholder);
                    remaining = &remaining[end + 2..];
                    counter += 1;
                } else {
                    result.push_str("$$");
                }
            }
            Some(("$", start)) => {
                result.push_str(&remaining[..start]);
                remaining = &remaining[start + 1..];

                // Find closing $ (not on newline, not $$)
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
                    let math = &remaining[..end_idx];
                    if !math.trim().is_empty() {
                        let placeholder = format!("\u{FFFC}MATH{}I\u{FFFC}", counter);
                        blocks.push((placeholder.clone(), format!("${}$", math)));
                        result.push_str(&placeholder);
                        counter += 1;
                    } else {
                        result.push('$');
                        result.push_str(&remaining[..end_idx + 1]);
                    }
                    remaining = &remaining[end_idx + 1..];
                } else {
                    result.push('$');
                }
            }
            Some(("\\(", start)) => {
                result.push_str(&remaining[..start]);
                remaining = &remaining[start + 2..]; // skip \(

                if let Some(end) = remaining.find("\\)") {
                    let math = &remaining[..end];
                    if !math.trim().is_empty() {
                        // Store as $...$ format (inline) for consistency
                        let placeholder = format!("\u{FFFC}MATH{}I\u{FFFC}", counter);
                        blocks.push((placeholder.clone(), format!("${}$", math)));
                        result.push_str(&placeholder);
                        counter += 1;
                    }
                    remaining = &remaining[end + 2..]; // skip \)
                } else {
                    result.push_str("\\(");
                }
            }
            _ => {
                // Should never happen
                result.push_str(remaining);
                break;
            }
        }
    }

    (result, blocks)
}

/// Restore math placeholders with original content.
fn restore_math(text: &str, blocks: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (placeholder, original) in blocks {
        result = result.replace(placeholder, original);
    }
    result
}
