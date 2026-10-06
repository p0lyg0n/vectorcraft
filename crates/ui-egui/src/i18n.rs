//! Interface translations. Command ids, document text and file names remain stable: only what is
//! drawn is translated, so shortcuts, the control channel, MCP, journals and tests keep working on
//! the English labels. Untranslated labels fall back to English so coverage can grow incrementally.
//!
//! The catalogues live in `crates/ui-egui/locales/<code>.tsv` (see the header of `ja.tsv` for the
//! format): one English source text and its translation per line, optionally inside a `@context`
//! section for words whose translation depends on where they appear ("Type" is the 書式 menu but
//! the テキスト tab of Document Setup). Lines whose source contains `{}` are templates for labels
//! composed at run time ("Undo {}" → "{}の取り消し"); the part matched by `{}` is translated in turn.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ja,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::En, Self::Ja];

    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ja => "日本語",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.code() == code)
    }

    fn catalog(self) -> Option<&'static Catalog> {
        match self {
            Self::En => None,
            Self::Ja => Some(&JAPANESE),
        }
    }

    /// `text` in this language (`text` itself when there is no translation).
    pub fn tr(self, text: &str) -> &str {
        self.tr_in("", text)
    }

    /// `text` as it reads in `context` (a `@context` section of the catalogue), falling back to the
    /// context-free translation, then to `text`.
    pub fn tr_in<'a>(self, context: &str, text: &'a str) -> &'a str {
        match self.catalog().and_then(|c| c.exact(context, text)) {
            Some(t) => t,
            None => text,
        }
    }

    /// Like [`Self::tr_in`], also translating labels composed at run time through the catalogue's
    /// templates ("Undo Move" → "移動の取り消し").
    pub fn tr_owned(self, context: &str, text: &str) -> String {
        let Some(c) = self.catalog() else { return text.to_string() };
        if let Some(t) = c.exact(context, text) {
            return t.to_string();
        }
        c.templated(context, text, 0).unwrap_or_else(|| text.to_string())
    }
}

thread_local! {
    /// The language the UI on this thread draws in: set from `UiState::language` at the start of
    /// every frame, so widgets that don't see the app (panels' helpers, dialogs) can translate.
    /// Per thread so parallel tests don't see each other's language.
    static CURRENT: Cell<Language> = const { Cell::new(Language::En) };
}

thread_local! {
    /// Font menus show families by their Japanese names (Preferences ▸ Type ▸ Show Font Names in
    /// English is off): set every frame from the preference.
    static LOCAL_FONT_NAMES: Cell<bool> = const { Cell::new(false) };
}

/// Show families by their Japanese names in font menus on this thread (or not).
pub fn set_local_font_names(on: bool) {
    LOCAL_FONT_NAMES.with(|c| c.set(on));
}

/// Do font menus show Japanese family names on this thread?
pub fn local_font_names() -> bool {
    LOCAL_FONT_NAMES.with(Cell::get)
}

/// How font menus name `family`: its Japanese name when it has one and they show them.
pub fn font_label(family: &str) -> std::borrow::Cow<'_, str> {
    if LOCAL_FONT_NAMES.with(Cell::get)
        && let Some(local) = vectorcraft_text::FontDb::global().local_name(family)
    {
        return local.into();
    }
    family.into()
}

/// Make `lang` the language [`t`] and [`t_in`] translate to on this thread.
pub fn set_current(lang: Language) {
    CURRENT.with(|c| c.set(lang));
}

/// The language the UI on this thread draws in.
pub fn current() -> Language {
    CURRENT.with(Cell::get)
}

/// `text` in the current UI language.
pub fn t(text: &str) -> &str {
    current().tr(text)
}

/// `text` in the current UI language, as it reads in `context`.
pub fn t_in<'a>(context: &str, text: &'a str) -> &'a str {
    current().tr_in(context, text)
}

/// `text` (possibly composed at run time) in the current UI language.
pub fn t_owned(text: &str) -> String {
    current().tr_owned("", text)
}

/// [`t_owned`] in `context`.
pub fn t_owned_in(context: &str, text: &str) -> String {
    current().tr_owned(context, text)
}

/// A parsed catalogue.
#[derive(Default)]
pub(crate) struct Catalog {
    /// (context, source) → translation; "" is the context-free section.
    exact: HashMap<(&'static str, &'static str), &'static str>,
    /// Templates: (context, prefix, suffix, translation with one `{}`).
    templates: Vec<(&'static str, &'static str, &'static str, &'static str)>,
}

/// How deep templates nest ("Undo Show Rulers" is two levels).
const MAX_TEMPLATE_DEPTH: usize = 3;

impl Catalog {
    pub(crate) fn parse(src: &'static str) -> Self {
        let mut cat = Catalog::default();
        let mut context = "";
        for line in src.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(ctx) = line.strip_prefix('@') {
                context = ctx.trim();
                continue;
            }
            let Some((source, translation)) = line.split_once('\t') else { continue };
            let translation = translation.trim();
            if source.is_empty() || translation.is_empty() {
                continue;
            }
            if let Some((prefix, suffix)) = source.split_once("{}") {
                if translation.contains("{}") {
                    cat.templates.push((context, prefix, suffix, translation));
                }
                continue;
            }
            cat.exact.insert((context, source), translation);
        }
        // Longest pattern first, so "Undo Paste in Front {}"-style specific templates win.
        cat.templates.sort_by_key(|(_, p, s, _)| std::cmp::Reverse(p.len() + s.len()));
        cat
    }

    fn exact(&self, context: &str, text: &str) -> Option<&'static str> {
        if !context.is_empty()
            && let Some(t) = self.exact.get(&(context, text))
        {
            return Some(t);
        }
        self.exact.get(&("", text)).copied()
    }

    fn templated(&self, context: &str, text: &str, depth: usize) -> Option<String> {
        if depth >= MAX_TEMPLATE_DEPTH {
            return None;
        }
        for (ctx, prefix, suffix, translation) in &self.templates {
            if !ctx.is_empty() && *ctx != context {
                continue;
            }
            let Some(inner) = text.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix)) else { continue };
            if inner.is_empty() || text.len() < prefix.len() + suffix.len() {
                continue;
            }
            let inner = match self.exact(context, inner) {
                Some(t) => t.to_string(),
                None => self.templated(context, inner, depth + 1).unwrap_or_else(|| inner.to_string()),
            };
            return Some(translation.replacen("{}", &inner, 1));
        }
        None
    }
}

static JAPANESE: LazyLock<Catalog> = LazyLock::new(|| Catalog::parse(include_str!("../locales/ja.tsv")));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_contexts_templates_and_skips_junk() {
        let cat = Catalog::parse(
            "# comment\nType\t書式\n\n@Document Setup\nType\tテキスト\nbroken line\nUndo {}\t{}の取り消し\n@\nShow {}\t{}を表示\nRulers\t定規\n",
        );
        assert_eq!(cat.exact("", "Type"), Some("書式"));
        assert_eq!(cat.exact("Document Setup", "Type"), Some("テキスト"));
        assert_eq!(cat.exact("Layers", "Type"), Some("書式"), "unknown contexts fall back to the plain entry");
        assert_eq!(cat.exact("", "broken line"), None);
        assert_eq!(cat.templated("", "Show Rulers", 0).as_deref(), Some("定規を表示"));
        // The Undo template lives in the Document Setup section only.
        assert_eq!(cat.templated("", "Undo Rulers", 0), None);
        assert_eq!(cat.templated("Document Setup", "Undo Rulers", 0).as_deref(), Some("定規の取り消し"));
        assert_eq!(cat.templated("", "Show ", 0), None, "an empty hole matches nothing");
    }

    #[test]
    fn templates_nest_and_untranslated_holes_stay_english() {
        let cat = Catalog::parse("Undo {}\t{}の取り消し\nShow {}\t{}を表示\nRulers\t定規\n");
        assert_eq!(cat.templated("", "Undo Show Rulers", 0).as_deref(), Some("定規を表示の取り消し"));
        assert_eq!(cat.templated("", "Undo Frobnicate", 0).as_deref(), Some("Frobnicateの取り消し"));
    }

    #[test]
    fn translations_preserve_unknown_text() {
        assert_eq!(Language::En.tr("File"), "File");
        assert_eq!(Language::Ja.tr("File"), "ファイル");
        assert_eq!(Language::Ja.tr("日本語の文書.vectorcraft"), "日本語の文書.vectorcraft");
        assert_eq!(Language::Ja.tr_owned("", "Undo Move"), "移動の取り消し");
        assert_eq!(Language::En.tr_owned("", "Undo Move"), "Undo Move");
        assert_eq!(Language::parse("xx"), None);
        assert_eq!(Language::parse("ja"), Some(Language::Ja));
    }

    #[test]
    fn japanese_catalogue_is_well_formed() {
        let src = include_str!("../locales/ja.tsv");
        let mut seen = std::collections::HashSet::new();
        let mut context = "";
        for (n, line) in src.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(c) = line.strip_prefix('@') {
                context = c.trim();
                continue;
            }
            let (en, ja) = line.split_once('\t').unwrap_or_else(|| panic!("line {}: no tab: {line:?}", n + 1));
            assert!(!ja.trim().is_empty(), "line {}: empty translation", n + 1);
            assert!(!ja.contains('\t'), "line {}: more than one tab", n + 1);
            assert_eq!(en.contains("{}"), ja.contains("{}"), "line {}: template holes differ", n + 1);
            assert!(seen.insert((context, en)), "line {}: {en:?} is listed twice in @{context}", n + 1);
            // An ellipsis on the English side stays on the Japanese side (it means "opens a dialog").
            assert_eq!(en.ends_with('…'), ja.ends_with('…'), "line {}: ellipsis differs: {en:?} → {ja:?}", n + 1);
        }
    }

    #[test]
    fn menus_translate_in_their_own_context() {
        assert_eq!(Language::Ja.tr_in("Object", "Arrange"), "重ね順");
        assert_eq!(Language::Ja.tr_in("Window", "Arrange"), "アレンジ");
    }

    /// Every label the menu bar draws has a Japanese translation, except names (fonts, plug-ins,
    /// effects, libraries, workspaces) that come from data rather than the code.
    #[test]
    fn every_static_menu_label_is_translated() {
        use crate::menus::Item;
        fn walk(items: &[Item], path: &str, missing: &mut Vec<String>) {
            for it in items {
                let (label, children) = match it {
                    Item::Cmd(l, id, _) => {
                        // Generated lists: names, not interface text.
                        if matches!(
                            *id,
                            "app.language" | "window.brightness" | "type.font" | "window.workspace" | "file.openRecent" | "view.savedView"
                        ) || id.starts_with("plugins.")
                            || id.starts_with("effect.")
                            || id.starts_with("window.library")
                        {
                            continue;
                        }
                        (*l, None)
                    }
                    Item::Todo(l, _) | Item::Header(l) => (*l, None),
                    Item::Sub(l, c) => (*l, Some(c)),
                    Item::Sep => continue,
                };
                // Product and format names read the same in Japanese.
                let same = matches!(label, "OpenType");
                // Labels translate in the context of their top-level menu.
                let menu = path.split(" > ").next().unwrap_or("");
                if Language::Ja.tr_owned(menu, label) == label && !label.is_empty() && !same {
                    missing.push(format!("{path} > {label}"));
                }
                // Lists of fonts, sizes, grid presets and libraries hold names, not interface text.
                let data_list = matches!(label, "Font" | "Recent Fonts" | "Size")
                    || label.ends_with("Point Perspective")
                    || label == "Swatch Libraries"
                    || label == "Graphic Style Libraries";
                if let Some(c) = children
                    && !data_list
                {
                    walk(c, &format!("{path} > {label}"), missing);
                }
            }
        }
        let mut missing = vec![];
        for (title, items) in crate::menus::menu_tree() {
            // The Effect menu lists the effect catalogue's names (translated with the effects).
            if title == "Effect" {
                continue;
            }
            if title != "VectorCraft" && Language::Ja.tr(title) == title {
                missing.push(title.to_string());
            }
            walk(&items, title, &mut missing);
        }
        assert!(missing.is_empty(), "{} menu labels have no Japanese:\n{}", missing.len(), missing.join("\n"));
    }

    /// In Japanese, a dialog draws its heading, labels and buttons in Japanese.
    #[test]
    fn dialogs_draw_in_the_interface_language() {
        let mut app = crate::VectorcraftApp::new(vectorcraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", serde_json::json!({"width": 100, "height": 100})).unwrap();
        crate::menus::invoke(&mut app, "file.documentSetup", serde_json::json!({}));
        assert!(app.ui.dialog.is_some());
        let english = crate::tests_labels::painted_text(&mut app, |app, ui| crate::dialogs::show(app, ui.ctx()));
        assert!(english.contains("Document Setup") && english.contains("Cancel"), "{english}");
        let japanese = crate::tests_labels::painted_text(&mut app, |app, ui| {
            set_current(Language::Ja);
            crate::dialogs::show(app, ui.ctx());
        });
        set_current(Language::En);
        for label in ["ドキュメント設定", "キャンセル", "OK"] {
            assert!(japanese.contains(label), "{label} in {japanese}");
        }
        assert!(!japanese.contains("Cancel"), "{japanese}");
    }

    /// Menu items not built yet are struck through, not just greyed.
    #[test]
    fn menu_items_not_built_yet_are_struck_through() {
        let mut app = crate::VectorcraftApp::new(vectorcraft_engine::Session::new(), crate::Services::default());
        let items = vec![crate::menus::Item::Todo("Design…", ""), crate::menus::Item::Cmd("Undo", "edit.undo", serde_json::Value::Null)];
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut struck = vec![];
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::menus::menu_body(&app, ui, "Object", &items, &mut None));
            out.textures_delta.clear();
            struck = crate::tests_labels::struck_text(&out);
        }
        let _ = &mut app;
        assert!(struck.iter().any(|(t, s)| t == "Design…" && *s), "{struck:?}");
        assert!(struck.iter().any(|(t, s)| t == "Undo" && !*s), "{struck:?}");
    }

    #[test]
    fn every_blending_mode_has_its_japanese_name() {
        for m in vectorcraft_color::BlendMode::ALL {
            assert_ne!(Language::Ja.tr(m.label()), m.label(), "{m:?}");
        }
        assert_eq!(Language::Ja.tr("Multiply"), "乗算");
    }

    #[test]
    fn current_language_is_per_thread() {
        set_current(Language::Ja);
        assert_eq!(t("File"), "ファイル");
        std::thread::spawn(|| assert_eq!(t("File"), "File")).join().unwrap();
        set_current(Language::En);
        assert_eq!(t("File"), "File");
    }

    #[test]
    fn language_command_validates_and_persists_without_a_document() {
        let mut app = crate::VectorcraftApp::new(vectorcraft_engine::Session::new(), crate::Services::default());
        crate::menus::run_ui_command(&mut app, "app.language", &serde_json::json!({"lang": "ja"})).unwrap().unwrap();
        assert_eq!(app.ui.language, Language::Ja);
        assert_eq!(crate::menus::checked(&app, "app.language", &serde_json::json!({"lang": "ja"})), Some(true));
        assert!(crate::menus::run_ui_command(&mut app, "app.language", &serde_json::json!({"lang": "xx"})).unwrap().is_err());
        assert_eq!(app.ui.language, Language::Ja);
        let saved = serde_json::to_string(&app.ui).unwrap();
        let restored: crate::state::UiState = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.language, Language::Ja);
    }
}
