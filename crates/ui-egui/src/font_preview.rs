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
    /// The starred families (UI state), and whether a list changed them this frame.
    static FAVORITES: RefCell<(Vec<String>, bool)> = const { RefCell::new((Vec::new(), false)) };
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

/// Keep the lists' favourites and the UI state's in step: a star clicked in a list this frame
/// goes to `state`, else `state` is what lists show.
pub fn sync_favorites(state: &mut Vec<String>) {
    FAVORITES.with(|f| {
        let mut f = f.borrow_mut();
        if std::mem::take(&mut f.1) {
            state.clone_from(&f.0);
        } else if f.0 != *state {
            f.0.clone_from(state);
        }
    });
}

pub fn is_favorite(family: &str) -> bool {
    FAVORITES.with(|f| f.borrow().0.iter().any(|x| x == family))
}

fn toggle_favorite(family: &str) {
    FAVORITES.with(|f| {
        let mut f = f.borrow_mut();
        match f.0.iter().position(|x| x == family) {
            Some(i) => {
                f.0.remove(i);
            }
            None => f.0.push(family.to_string()),
        }
        f.1 = true;
    });
}

/// The sample text for `family`, by what the family is for: Japanese for Japanese fonts (when
/// they have the kana), Han characters for Chinese and Korean ones, a word for the others.
pub fn sample_text(family: &str) -> &'static str {
    let db = FontDb::global();
    let face = db.face(family, "Regular").filter(|f| f.family.eq_ignore_ascii_case(family));
    let has = |s: &str| face.as_ref().is_some_and(|f| s.chars().filter(|c| !c.is_whitespace()).all(|c| f.covers(c)));
    match db.script(family) {
        vectorcraft_text::FontScript::Japanese if has("文字もじモジ") => "文字もじモジ",
        vectorcraft_text::FontScript::OtherCjk | vectorcraft_text::FontScript::Japanese if has("字體") => "字體様式",
        _ if has("Sample") => "Sample",
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

/// A font list row: the name at the left, the sample (when previews are on) in a column at the
/// right, and a star to (un)favourite the family. Returns the row's response.
pub fn row(ui: &mut Ui, family: &str, label: &str, selected: bool) -> egui::Response {
    let preview = ENABLED.with(Cell::get);
    let t = crate::theme::Tokens::get(ui.ctx());
    let h = 24.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(260.0), h), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    if selected || resp.hovered() {
        ui.painter().rect_filled(rect, 2.0, if selected { t.row_selected } else { t.hover });
    }
    let star = Rect::from_min_size(pos2(rect.right() - 22.0, rect.top() + 4.0), vec2(16.0, 16.0));
    let sample = Rect::from_min_size(pos2(rect.right() - 26.0 - SAMPLE_W, rect.top() + 2.0), vec2(SAMPLE_W, h - 4.0));
    // The name, clipped before the sample's column.
    let name_room = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top()), pos2(sample.left() - 6.0, rect.bottom()));
    ui.painter().with_clip_rect(name_room).text(name_room.left_center(), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.5), t.text);
    if preview {
        paint(ui, family, sample, t.text);
    }
    let fav = is_favorite(family);
    let sr = ui.interact(star, ui.id().with(("font-star", family)), egui::Sense::click());
    if fav || resp.hovered() || sr.hovered() {
        crate::icons::paint(ui, "star", star, if fav { t.accent } else { t.text_dim });
    }
    if sr.clicked() {
        toggle_favorite(family);
    }
    // A click on the star isn't a click on the row.
    if sr.clicked() { resp.clone().with_new_rect(Rect::NOTHING) } else { resp }
}

/// Width of the sample column.
const SAMPLE_W: f32 = 104.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_fonts_sample_latin_and_render() {
        assert_eq!(sample_text("Source Sans 3"), "Sample", "a Latin font samples a word");
        let doc = sample_doc("Source Sans 3", "Sample", 88.0, 20.0).unwrap();
        let img = vectorcraft_render::Renderer::new().render_region(&doc, vectorcraft_geom::Rect::new(0.0, 0.0, 88.0, 20.0), 1.0, false);
        assert!(img.pixels.chunks(4).any(|p| p[3] > 0), "the sample draws something");
    }

    #[test]
    fn japanese_fonts_sample_japanese() {
        // The bundled Shippori Mincho has kana and kanji.
        assert_eq!(sample_text("Shippori Mincho"), "文字もじモジ");
    }

    #[test]
    fn favorites_follow_the_ui_state_and_a_star_click() {
        let mut state = vec!["Inter".to_string()];
        sync_favorites(&mut state);
        assert!(is_favorite("Inter") && !is_favorite("Source Sans 3"));
        toggle_favorite("Source Sans 3");
        toggle_favorite("Inter");
        sync_favorites(&mut state);
        assert_eq!(state, ["Source Sans 3"], "the clicks reach the UI state");
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
