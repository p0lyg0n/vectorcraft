//! The All Tools drawer: every tool, group by group, as icons (a tooltip names each with its
//! shortcut) or as a list of icons and names; picking one selects it and closes the drawer.

use super::DialogSpec;
use crate::VectorcraftApp;
use crate::state::Dialog;
use crate::widgets;

pub(super) const SPEC: DialogSpec = DialogSpec { heading: |_| "All Tools".into(), body, ok: None, ..DialogSpec::FORM };

/// Icon size and spacing of the grid.
const ICON: f32 = 30.0;

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let list = d.bool("list");
    // Grid or list, as the drawer was last shown.
    ui.horizontal(|ui| {
        if widgets::icon_button(ui, "layout-grid", "Grid View", !list, 24.0).clicked() {
            d.fields.insert("list".into(), false.into());
        }
        if widgets::icon_button(ui, "menu", "List View", list, 24.0).clicked() {
            d.fields.insert("list".into(), true.into());
        }
    });
    ui.add_space(4.0);
    let current = app.session.tool_id();
    let mut picked = None;
    egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 220.0).max(160.0)).show(ui, |ui| {
        for g in vectorcraft_tools::TOOL_GROUPS {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if list {
                    for tool in g.iter() {
                        let r = ui.horizontal(|ui| {
                            let icon =
                                widgets::icon_button(ui, crate::icons::tool_icon(tool.icon), &crate::toolbar::tip(tool), tool.id == current, 24.0);
                            let name = ui.selectable_label(tool.id == current, crate::i18n::t(tool.label));
                            icon.clicked() || name.clicked()
                        });
                        if r.inner {
                            picked = Some(tool.id);
                        }
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for tool in g.iter() {
                            if widgets::icon_button(ui, crate::icons::tool_icon(tool.icon), &crate::toolbar::tip(tool), tool.id == current, ICON)
                                .clicked()
                            {
                                picked = Some(tool.id);
                            }
                        }
                    });
                }
            });
        }
    });
    if let Some(id) = picked {
        app.select_tool(id);
    }
    picked.is_some()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::VectorcraftApp;

    #[test]
    fn the_drawer_shows_tool_icons_with_names_in_tooltips_and_lists_names_on_request() {
        let mut app = VectorcraftApp::new(vectorcraft_engine::Session::new(), Default::default());
        app.run("file.new", json!({"width": 100, "height": 100})).unwrap();
        app.ui.dialog = Some(crate::state::Dialog::new("allTools", json!({})));
        let grid = crate::tests_labels::painted_text(&mut app, |app, ui| super::super::show(app, ui.ctx()));
        assert!(grid.contains("All Tools") && !grid.contains("Pen Tool"), "grid: icons only\n{grid}");
        app.ui.dialog.as_mut().unwrap().fields.insert("list".into(), json!(true));
        let list = crate::tests_labels::painted_text(&mut app, |app, ui| super::super::show(app, ui.ctx()));
        assert!(list.contains("Pen Tool"), "list: names\n{list}");
        // The tooltip names the tool with its shortcut.
        let pen = vectorcraft_tools::tool_info("pen").unwrap();
        assert!(crate::toolbar::tip(pen).starts_with("Pen Tool"));
    }
}
