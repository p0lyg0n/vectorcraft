//! The canvas's context menu (right-click): the commands for the selection at hand, from the menu
//! bar's commands. A right-click on an unselected object selects it first; on empty canvas the
//! menu offers what applies without a selection (Undo, Paste, Select All).

use serde_json::{Value, json};
use vectorcraft_geom::Point;

use crate::VectorcraftApp;
use crate::menus::{self, Item};

fn c(label: &'static str, id: &'static str) -> Item {
    Item::Cmd(label, id, Value::Null)
}

fn sub(label: &'static str, items: Vec<Item>) -> Item {
    Item::Sub(label, items)
}

/// The clipboard rows both menus start with: Cut, Copy, Paste and the other pastes.
fn clipboard() -> Vec<Item> {
    vec![
        c("Cut", "edit.cut"),
        c("Copy", "edit.copy"),
        c("Paste", "edit.paste"),
        sub(
            "Paste…",
            vec![
                c("Paste in Front", "edit.pasteInFront"),
                c("Paste in Back", "edit.pasteInBack"),
                c("Paste in Place", "edit.pasteInPlace"),
                c("Paste on All Artboards", "edit.pasteOnAllArtboards"),
            ],
        ),
        Item::Sep,
        c("Undo", "edit.undo"),
        c("Redo", "edit.redo"),
        Item::Sep,
    ]
}

fn select_sub() -> Item {
    sub(
        "Select",
        vec![
            c("All", "select.all"),
            c("All on Active Artboard", "select.allOnArtboard"),
            c("Deselect", "select.none"),
            c("Reselect", "select.reselect"),
            Item::Sep,
            c("Inverse", "select.inverse"),
            Item::Sep,
            c("Next Object Above", "select.nextAbove"),
            c("Next Object Below", "select.nextBelow"),
        ],
    )
}

/// The menu's items: for the selection, or (nothing selected) for the view.
pub fn items(app: &VectorcraftApp) -> Vec<Item> {
    let selected = app.session.active().is_some_and(|d| !d.selection.objects.is_empty());
    let mut v = clipboard();
    if !selected {
        // Show/Hide and Lock/Unlock read the current state (the menus' dynamic labels).
        v.extend([
            c("Zoom In", "view.zoomIn"),
            c("Zoom Out", "view.zoomOut"),
            Item::Sep,
            c("Show Rulers", "view.rulers"),
            c("Show Grid", "view.grid"),
            c("Hide Guides", "view.guides"),
            c("Lock Guides", "view.guides.lock"),
            Item::Sep,
            select_sub(),
            Item::Sep,
            c("Outline", "view.outline"),
        ]);
        return keep_known(v);
    }
    v.extend([
        c("Group", "object.group"),
        c("Ungroup", "object.ungroup"),
        c("Join", "path.join"),
        c("Average…", "path.average"),
        c("Simplify…", "object.path.simplify"),
        Item::Sep,
        c("Make Clipping Mask", "object.clippingMask.make"),
        c("Release Clipping Mask", "object.clippingMask.release"),
        c("Make Compound Path", "object.compoundPath.make"),
        c("Release Compound Path", "object.compoundPath.release"),
        c("Make Guides", "view.guides.make"),
        Item::Sep,
        sub(
            "Transform",
            vec![
                c("Transform Again", "object.transformAgain"),
                Item::Sep,
                c("Move…", "object.move"),
                c("Rotate…", "object.rotate"),
                c("Reflect…", "object.reflect"),
                c("Scale…", "object.scale"),
                c("Shear…", "object.shear"),
                Item::Sep,
                c("Transform Each…", "object.transformEach"),
                Item::Sep,
                c("Reset Bounding Box", "object.resetBoundingBox"),
            ],
        ),
        sub(
            "Arrange",
            vec![
                c("Bring to Front", "object.arrange.bringToFront"),
                c("Bring Forward", "object.arrange.bringForward"),
                c("Send Backward", "object.arrange.sendBackward"),
                c("Send to Back", "object.arrange.sendToBack"),
            ],
        ),
        select_sub(),
        Item::Sep,
        sub(
            "Collect for Export",
            vec![
                Item::Cmd("As Single Asset", "assets.add", json!({"multiple": false})),
                Item::Cmd("As Multiple Assets", "assets.add", json!({"multiple": true})),
            ],
        ),
        c("Export Selection…", "file.exportSelection"),
    ]);
    // Only commands the app has: an id the menus don't know drops out, so the menu never offers
    // a dead item.
    keep_known(v)
}

/// `items` without the commands the registry lacks (and the subs or separators left empty).
fn keep_known(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = vec![];
    for it in items {
        match it {
            Item::Cmd(_, id, _) if !menus::is_command(id) => {}
            Item::Sub(label, children) => {
                let children = keep_known(children);
                if children.iter().any(|c| !matches!(c, Item::Sep)) {
                    out.push(Item::Sub(label, children));
                }
            }
            Item::Sep if out.last().is_none_or(|l| matches!(l, Item::Sep)) => {}
            other => out.push(other),
        }
    }
    while out.last().is_some_and(|l| matches!(l, Item::Sep)) {
        out.pop();
    }
    out
}

/// A right-click at `doc_point`: select the object there unless it is selected already (Shift
/// adds nothing: the menu acts on what is clicked).
pub fn select_under(app: &mut VectorcraftApp, doc_point: Point, zoom: f64) {
    let Some(st) = app.session.active() else { return };
    let opt = vectorcraft_doc::hit::HitOptions { tol: 3.0 / zoom, outline: app.ui.view.outline, path_only: false };
    let Some(hit) = vectorcraft_doc::hit::hit_test(&st.doc, doc_point, opt) else { return };
    let id = hit.top_object(st.isolation);
    if st.selection.objects.contains(&id) {
        return;
    }
    if let Err(e) = app.run("select.set", json!({"ids": [id.0]})) {
        app.status(e);
    }
}

/// Draw the menu (inside egui's context-menu popup) and run what is clicked.
pub fn show(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let items = items(app);
    let mut clicked = None;
    menus::menu_body(app, ui, "Object", &items, &mut clicked);
    if let Some((id, p)) = clicked {
        menus::invoke(app, &id, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::Session;

    fn app() -> VectorcraftApp {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
        app
    }

    fn ids(items: &[Item]) -> Vec<&'static str> {
        let mut v = vec![];
        for it in items {
            match it {
                Item::Cmd(_, id, _) => v.push(*id),
                Item::Sub(_, c) => v.extend(ids(c)),
                _ => {}
            }
        }
        v
    }

    #[test]
    fn every_item_is_a_command_the_app_has() {
        let mut app = app();
        app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 50, "height": 50})).unwrap();
        let items = items(&app);
        let ids = ids(&items);
        assert!(ids.contains(&"object.clippingMask.make"), "{ids:?}");
        for id in &ids {
            assert!(menus::is_command(id), "{id}");
        }
        assert!(!matches!(items.last(), Some(Item::Sep)));
    }

    #[test]
    fn without_a_selection_it_offers_paste_and_select_all() {
        let app = app();
        let ids = ids(&items(&app));
        for id in ["edit.paste", "edit.undo", "view.zoomIn", "view.rulers", "view.guides.lock", "select.all", "view.outline"] {
            assert!(ids.contains(&id), "{id} in {ids:?}");
        }
        assert!(!ids.contains(&"object.clippingMask.make"));
    }

    #[test]
    fn right_click_selects_the_object_under_it_and_the_menu_makes_a_clipping_mask() {
        let mut app = app();
        app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 100, "height": 100})).unwrap();
        app.run("shape.ellipse", json!({"x": 30, "y": 30, "width": 40, "height": 40})).unwrap();
        app.run("select.all", json!({})).unwrap();
        // A right-click on a selected object keeps the selection.
        select_under(&mut app, Point::new(50.0, 50.0), 1.0);
        assert_eq!(app.session.active().unwrap().selection.objects.len(), 2);
        // Make Clipping Mask from the menu: the top object clips the one below.
        menus::invoke(&mut app, "object.clippingMask.make", Value::Null);
        let st = app.session.active().unwrap();
        assert_eq!(st.selection.objects.len(), 1, "the clip group is selected");
        let group = st.doc.node(st.selection.objects[0]).unwrap();
        assert!(group.kind_label().contains("Clip"), "{}", group.kind_label());
        // On an unselected object, a right-click selects it.
        app.run("select.none", json!({})).unwrap();
        select_under(&mut app, Point::new(50.0, 50.0), 1.0);
        assert_eq!(app.session.active().unwrap().selection.objects.len(), 1);
    }

    #[test]
    fn labels_translate_in_japanese() {
        assert_eq!(crate::i18n::Language::Ja.tr_in("Object", "Make Clipping Mask"), "クリッピングマスクを作成");
    }
}
