//! The window in Japanese: what it still draws in English. The main screens (home, a document with
//! nothing and something selected, every docked and icon panel) are drawn headless in Japanese, and
//! every painted line made only of untranslated English words is reported.

use std::collections::BTreeSet;

use egui::{Pos2, vec2};
use serde_json::json;
use vectorcraft_engine::Session;

use crate::VectorcraftApp;
use crate::i18n::Language;

/// Painted lines of two frames of the whole 1440×900 window.
fn window_text(app: &mut VectorcraftApp) -> Vec<String> {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))), ..Default::default() };
    let mut text = String::new();
    for i in 0..3 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        if i == 2 {
            text = crate::tests_labels::shapes_text(&out);
        }
    }
    text.lines().map(str::to_string).collect()
}

/// Is `line` English the catalogue has no entry for? Names, numbers and units aren't.
fn untranslated(line: &str) -> bool {
    let words: Vec<&str> = line.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| w.len() > 1).collect();
    if words.is_empty() || line.chars().any(|c| !c.is_ascii() && c.is_alphabetic()) {
        return false;
    }
    // Units, file and product names that stay as they are.
    const KEEP: &[&str] =
        &["mm", "pt", "px", "in", "cm", "pc", "RGB", "CMYK", "VectorCraft", "Untitled", "Discord", "OK", "fps", "ms", "ui", "render"];
    !words.iter().all(|w| KEEP.contains(w))
}

#[test]
#[ignore = "a report, not a check: cargo test -p vectorcraft-ui-egui ja_coverage -- --ignored --nocapture"]
fn ja_coverage_report() {
    let mut app = VectorcraftApp::new(Session::new(), Default::default());
    app.ui.language = Language::Ja;
    let mut found = BTreeSet::new();
    let mut take = |app: &mut VectorcraftApp, what: &str| {
        for l in window_text(app).into_iter().filter(|l| untranslated(l)) {
            found.insert(format!("{l}\t[{what}]"));
        }
    };
    take(&mut app, "home");
    app.run("file.new", json!({"width": 800, "height": 600})).unwrap();
    take(&mut app, "document");
    app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 50, "height": 50})).unwrap();
    take(&mut app, "selection");
    for (id, _, _) in crate::state::ICON_PANELS {
        app.ui.open_panel = Some((*id).to_string());
        take(&mut app, id);
    }
    app.ui.open_panel = None;
    let mut seen = BTreeSet::new();
    for l in &found {
        let text = l.split('\t').next().unwrap_or("");
        if seen.insert(text.to_string()) {
            println!("{l}");
        }
    }
    println!("{} untranslated lines", seen.len());
}
