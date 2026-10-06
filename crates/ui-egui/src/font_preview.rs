//! Font samples in the font lists: each family's name drawn next to a sample set in that font, in
//! Japanese for fonts that have kana ("あア永"), else "Ag". Samples are rendered by the canvas's own
//! text engine to small textures, only for rows on screen, cached, and at most a few new ones a
//! frame, so a long list scrolls without stalling while the fonts load.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use egui::{Rect, TextureHandle, Ui, pos2, vec2};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{CharStyle, Document, Node, NodeKind, TextObject};
use vectorcraft_geom::Point;
use vectorcraft_text::FontDb;

/// New samples rendered per frame; rows waiting show their name only until the next frames.
const PER_FRAME: u32 = 3;
/// Samples kept (textures are small: a few kilobytes each).
const CACHE_MAX: usize = 400;

thread_local! {
    /// Preferences ▸ Type ▸ Enable in-menu font previews, set every frame.
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static CACHE: RefCell<HashMap<(String, u32), Option<TextureHandle>>> = RefCell::new(HashMap::new());
    /// (frame, samples rendered in it).
    static BUDGET: Cell<(u64, u32)> = const { Cell::new((0, 0)) };
}

/// Show samples in the font lists on this thread (or not).
pub fn set_enabled(on: bool) {
    ENABLED.with(|c| c.set(on));
}

/// The sample text for `family`: Japanese when its fonts have kana and kanji, else Latin.
pub fn sample_text(family: &str) -> &'static str {
    let db = FontDb::global();
    match db.face(family, "Regular") {
        Some(f) if f.family.eq_ignore_ascii_case(family) && ['あ', 'ア', '永'].iter().all(|c| f.covers(*c)) => "あア永",
        _ => "Ag",
    }
}

/// A one-line document of `text` in `family`, white, `w`×`h` pt.
fn sample_doc(family: &str, text: &str, w: f64, h: f64) -> Option<Document> {
    let mut doc = Document::new(w, h);
    let size = h * 0.72;
    let style = CharStyle { font_family: family.to_string(), size, fill: Paint::solid(Color::WHITE), stroke: Paint::None, ..CharStyle::default() };
    let t = TextObject::point(Point::new(1.0, h * 0.78), text, style);
    let (id, l) = (doc.alloc_id(), doc.layers.first()?.id);
    doc.insert(Some(l), 0, Node::new(id, NodeKind::Text(Box::new(t)))).ok()?;
    Some(doc)
}

/// Draw the sample of `family` in `rect` (tinted `color`), if it is ready or this frame can render
/// it. Returns whether it was drawn.
pub fn paint(ui: &Ui, family: &str, rect: Rect, color: egui::Color32) -> bool {
    let ppp = ui.ctx().pixels_per_point();
    let key = (family.to_string(), (rect.height() * ppp).round() as u32);
    let cached = CACHE.with(|c| c.borrow().get(&key).cloned());
    let tex = match cached {
        Some(t) => t,
        None => {
            let frame = ui.ctx().cumulative_frame_nr();
            let (f, n) = BUDGET.with(Cell::get);
            let n = if f == frame { n } else { 0 };
            if n >= PER_FRAME {
                // The rest wait for the next frames.
                ui.ctx().request_repaint();
                return false;
            }
            BUDGET.with(|b| b.set((frame, n + 1)));
            let text = sample_text(family);
            let tex = crate::widgets::doc_preview(ui, &format!("font-sample:{family}"), rect.size(), |w, h| sample_doc(family, text, w, h));
            CACHE.with(|c| {
                let mut c = c.borrow_mut();
                if c.len() > CACHE_MAX {
                    c.clear();
                }
                c.insert(key, tex.clone());
            });
            tex
        }
    };
    let Some(tex) = tex else { return false };
    ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    true
}

/// A font list row: the name, and the sample to its right when previews are on. Returns the
/// row's response.
pub fn row(ui: &mut Ui, family: &str, label: &str, selected: bool) -> egui::Response {
    let preview = ENABLED.with(Cell::get);
    let width = ui.available_width();
    let h = 24.0;
    let resp = ui.add_sized(vec2(width, h), egui::Button::selectable(selected, label));
    if preview && ui.is_rect_visible(resp.rect) {
        let r = Rect::from_min_size(pos2(resp.rect.right() - 92.0, resp.rect.top() + 2.0), vec2(88.0, h - 4.0));
        let t = crate::theme::Tokens::get(ui.ctx());
        paint(ui, family, r, t.text);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_fonts_sample_latin_and_render() {
        assert_eq!(sample_text("Source Sans 3"), "Ag", "no kana in Source Sans 3");
        let doc = sample_doc("Source Sans 3", "Ag", 88.0, 20.0).unwrap();
        let img = vectorcraft_render::Renderer::new().render_region(&doc, vectorcraft_geom::Rect::new(0.0, 0.0, 88.0, 20.0), 1.0, false);
        assert!(img.pixels.chunks(4).any(|p| p[3] > 0), "the sample draws something");
    }

    #[test]
    fn japanese_fonts_sample_japanese() {
        // The bundled Shippori Mincho has kana and kanji.
        assert_eq!(sample_text("Shippori Mincho"), "あア永");
    }

    #[test]
    fn only_a_few_samples_render_per_frame() {
        let ctx = egui::Context::default();
        let mut drawn = 0;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in 0..10 {
                let r = Rect::from_min_size(pos2(0.0, i as f32 * 20.0), vec2(80.0, 18.0));
                if paint(ui, &format!("No Such Family {i}"), r, egui::Color32::WHITE) {
                    drawn += 1;
                }
            }
        });
        out.textures_delta.clear();
        assert!(drawn <= PER_FRAME as usize, "{drawn} rendered in one frame");
    }
}
