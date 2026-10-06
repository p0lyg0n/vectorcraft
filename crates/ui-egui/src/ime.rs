//! Input methods (Japanese, Chinese, Korean…) for the Type tool on the canvas.
//!
//! The canvas isn't an egui `TextEdit`, so it asks for the input method itself: while the Type
//! tool edits text, every frame reports the caret to egui (which enables the input method and
//! places its candidate window there). The text being composed is drawn inline at the caret,
//! underlined, and reaches the document only when it is committed. While composing, the editing
//! keys (Enter, Backspace, arrows, Escape) belong to the input method.

use egui::{Pos2, Rect, Stroke, pos2, vec2};
use vectorcraft_geom::Point;

use crate::VectorcraftApp;

/// Text being composed by the input method (empty when not composing).
#[derive(Default)]
pub struct Ime {
    pub preedit: String,
}

impl Ime {
    /// Is the input method composing text (so editing keys are its, not the tool's)?
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }
}

/// Take this frame's input-method events: committed text goes to the Type tool, composed text is
/// kept for drawing. Returns the committed strings (for tests).
pub(crate) fn route(app: &mut VectorcraftApp, ctx: &egui::Context) -> Vec<String> {
    let mut commits = vec![];
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let egui::Event::Ime(ev) = e else { return true };
            match ev {
                egui::ImeEvent::Preedit { text, .. } => app.ime.preedit = text.clone(),
                egui::ImeEvent::Commit(text) => {
                    app.ime.preedit.clear();
                    // Enter that ends a composition arrives as a newline commit on some
                    // platforms: it isn't text.
                    if !text.is_empty() && text != "\n" && text != "\r" {
                        commits.push(text.clone());
                    }
                }
                #[allow(deprecated)]
                egui::ImeEvent::Disabled => app.ime.preedit.clear(),
                _ => {}
            }
            false
        })
    });
    let view = app.view_info();
    for t in &commits {
        let r = app.session.tool_text(t, view);
        crate::canvas::apply_requests(app, r);
    }
    commits
}

/// While the Type tool edits text: tell egui where the caret is (enabling the input method there)
/// and draw the composition inline. `to_screen` maps document points to the screen.
pub(crate) fn show(app: &mut VectorcraftApp, ctx: &egui::Context, painter: &egui::Painter, canvas: Rect, to_screen: impl Fn(Point) -> Pos2) {
    if !app.session.tool_wants_text() {
        app.ime.preedit.clear();
        return;
    }
    let view = app.view_info();
    let Some((a, b)) = app.session.tool_caret(view) else { return };
    let (a, b) = (to_screen(a), to_screen(b));
    if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
        return;
    }
    let cursor = Rect::from_two_pos(a, b).expand2(vec2(1.0, 0.0));
    ctx.output_mut(|o| {
        o.ime = Some(egui::output::IMEOutput {
            purpose: egui::IMEPurpose::Normal,
            rect: canvas,
            cursor_rect: cursor,
            should_interrupt_composition: false,
        })
    });
    if app.ime.composing() {
        draw_preedit(painter, &app.ime.preedit, a, b);
    }
}

/// The composition at the caret: on a paper-coloured box, at the caret's height, underlined.
fn draw_preedit(painter: &egui::Painter, text: &str, a: Pos2, b: Pos2) {
    let height = (b - a).length().clamp(10.0, 96.0);
    let font = egui::FontId::proportional(height * 0.8);
    let galley = painter.layout_no_wrap(text.to_owned(), font, egui::Color32::BLACK);
    let top = pos2(a.x.min(b.x), a.y.min(b.y));
    let rect = Rect::from_min_size(top, vec2(galley.size().x, height));
    painter.rect_filled(rect.expand(1.0), 1.0, egui::Color32::WHITE);
    painter.galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, egui::Color32::BLACK);
    let y = rect.bottom() - 1.0;
    painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, egui::Color32::BLACK));
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vectorcraft_doc::NodeKind;
    use vectorcraft_engine::{Session, ViewInfo};
    use vectorcraft_tools::{PointerEvent, PointerKind};

    use super::*;

    fn app() -> VectorcraftApp {
        let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
        app.session.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
        app
    }

    /// An app whose Type tool edits a new point type object holding "A".
    fn app_typing() -> VectorcraftApp {
        let mut app = app();
        let v = ViewInfo::default();
        app.session.select_tool("type", v).unwrap();
        for kind in [PointerKind::Down, PointerKind::Up] {
            app.session.pointer(&PointerEvent::new(kind, 100.0, 100.0), v).unwrap();
        }
        app.session.tool_text("A", v).unwrap();
        app
    }

    fn frame(app: &mut VectorcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            route(app, ui.ctx());
            let canvas = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
            let painter = ui.painter().clone();
            show(app, ui.ctx(), &painter, canvas, |p| pos2(p.x as f32, p.y as f32));
        });
        out.textures_delta.clear();
        out
    }

    fn texts(app: &VectorcraftApp) -> Vec<String> {
        let mut v = vec![];
        app.session.doc().unwrap().doc.walk(|n| {
            if let NodeKind::Text(t) = &n.kind {
                v.push(t.plain_text());
            }
        });
        v
    }

    #[test]
    fn composition_is_shown_and_only_the_commit_is_typed() {
        let mut app = app_typing();
        assert!(app.session.tool_wants_text(), "the Type tool edits the text");
        let ctx = egui::Context::default();
        let out = frame(&mut app, &ctx, vec![]);
        let ime = out.platform_output.ime.expect("the input method is enabled at the caret");
        assert!(ime.cursor_rect.height() > 0.0);
        // Composing: nothing reaches the document yet.
        frame(&mut app, &ctx, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "にほんご".into(), active_range_chars: None })]);
        assert!(app.ime.composing());
        assert_eq!(texts(&app), ["A"]);
        // Committed: the converted text is typed at the caret.
        frame(&mut app, &ctx, vec![egui::Event::Ime(egui::ImeEvent::Commit("日本語".into()))]);
        assert!(!app.ime.composing());
        assert_eq!(texts(&app), ["A日本語"]);
        // A newline commit (Enter ending a composition) types nothing.
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Ime(egui::ImeEvent::Commit(
                "
"
                .into(),
            ))],
        );
        assert_eq!(texts(&app), ["A日本語"]);
    }

    #[test]
    fn no_input_method_without_text_editing() {
        let mut app = app();
        let ctx = egui::Context::default();
        let out = frame(&mut app, &ctx, vec![]);
        assert!(out.platform_output.ime.is_none());
    }
}
