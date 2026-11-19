//! The quick tools at the left of the navigation bar: Select, Hand and the Comment group (a note,
//! a text box, a callout, an attachment). Only while they apply: Edit text & images in the Edit
//! workspace, and the controls of a comment or Fill & Sign tool in use (colour, opacity, keeping
//! the tool, a text's size and border). There is no
//! floating toolbar: the other tools are in the Tools panel, the menus and the command search.
use egui::{Rect, Sense, Stroke, vec2};
use pdfcraft_engine::Edit;

use crate::{PdfCraftApp, QuickTool, comments, icons, theme, theme::Tokens};

/// The quick tools' buttons, in the navigation bar's 30 pt cells.
const CELL: f32 = 30.0;
/// The Comment group, of [`comments::GROUPS`].
const COMMENT_GROUP: usize = 0;

/// Select, Hand and the Comment group, then the controls of the tool in use, left to right.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp) {
    let t = Tokens::get(ui.ctx());
    if icons::button(ui, "mouse-pointer-2", CELL, app.quick_tool == QuickTool::Select, tl!("Select (V)")).clicked() {
        app.quick_tool = QuickTool::Select;
    }
    if icons::button(ui, "hand", CELL, app.quick_tool == QuickTool::Hand, tl!("Hand (H)")).clicked() {
        app.quick_tool = QuickTool::Hand;
    }
    comment_group(ui, app, &t);
    // Edit text & images, only while the Edit workspace is open or the tool is in use: the way
    // back to it there after Select or Hand.
    if app.mode == crate::Mode::Edit || app.quick_tool == QuickTool::EditText {
        let on = app.quick_tool == QuickTool::EditText;
        if icons::button(ui, "text-cursor-input", CELL, on, tl!("Edit text & images")).clicked() {
            if on {
                app.put_down_edit_text();
            } else {
                app.quick_tool = QuickTool::EditText;
            }
        }
    }
    active_tool_controls(ui, app, &t);
}

/// The Comment group's button: a click picks its last-used tool, a second click (or a
/// right-click) opens the group below it.
fn comment_group(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let Some(group) = comments::GROUPS.get(COMMENT_GROUP) else { return };
    let Some(&current) = app.comment_prefs.group_tool.get(COMMENT_GROUP) else { return };
    let active = matches!(app.quick_tool, QuickTool::Comment(tool) if tool.group() == COMMENT_GROUP);
    let resp = icons::button(ui, current.icon(), CELL, active, tl!(current.label()));
    // A small corner triangle marks the group.
    let r = resp.rect;
    let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
    ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { t.accent } else { t.text_faint }, Stroke::NONE));
    let open = (resp.clicked() && active) || resp.secondary_clicked();
    if resp.clicked() && !active {
        app.execute(current.command());
    }
    egui::Popup::menu(&resp).open_memory(open.then_some(egui::SetOpenCommand::Toggle)).align(egui::RectAlign::BOTTOM_START).gap(6.0).show(|ui| {
        for tool in group.iter() {
            let on = app.quick_tool == QuickTool::Comment(*tool);
            let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
            let press = theme::Press::track(ui, &click);
            press.wash(ui, row, 4, on);
            let body = row.translate(press.offset());
            icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
            let reserve = if on { 28.0 } else { 8.0 };
            crate::widgets::paint_left(
                ui,
                body.left_center() + vec2(34.0, 0.0),
                tl!(tool.label()),
                theme::regular(13.0),
                t.text,
                (row.width() - 34.0 - reserve).max(0.0),
            );
            if on {
                icons::paint(ui, Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)), "check", 14.0, t.accent);
            }
            let click = theme::hand(click);
            let info = tl!(tool.label()).to_string();
            click.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, info.clone()));
            if click.clicked() {
                app.execute(tool.command());
                ui.close();
            }
        }
    });
}

/// A thin rule between the quick tools and the controls of the tool in use.
fn controls_divider(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, CELL), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range().shrink(4.0), Stroke::new(1.0, t.divider));
}

fn selected_fill_text(app: &PdfCraftApp) -> Option<(usize, usize)> {
    let (index, id) = app.active_ids()?;
    let (page, annotation) = app.views.get(index)?.comments.selected?;
    let doc = app.session.get(id).filter(|doc| doc.allows_annotation())?;
    doc.info
        .annotations
        .iter()
        .any(|a| a.page == page && a.index == annotation && a.fill_sign && a.subtype == "FreeText" && !a.locked)
        .then_some((page, annotation))
}

/// Fill & Sign's Border controls for typed text: its width, colour and shape, for the selected
/// text (`picked`), or for new text when none is selected. The width's label is its tooltip.
fn fill_text_border_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, picked: Option<(usize, usize)>) {
    // The selected text's own border (Comment Properties may have changed it
    // since): the controls below show it and change only it. Without one they
    // set what new text gets until PdfCraft quits; Make Current Properties
    // Default on typed text keeps it.
    let selected = picked.and_then(|(page, annotation)| {
        let p = app.active_ids().and_then(|(_, id)| app.session.get(id))?.comment_props(page, annotation)?;
        let own = crate::fill_sign::TextBorder { width: p.width.unwrap_or(0.0), color: p.border_color, shape: p.border_shape.unwrap_or_default() };
        Some((page, annotation, own))
    });
    let was = selected.map_or(
        crate::fill_sign::TextBorder { width: app.fill_text_border, color: app.fill_text_border_color, shape: app.fill_text_border_shape },
        |(_, _, own)| own,
    );
    let mut border = was;
    // A border around the text, in its colour; the text keeps its place.
    let border_label = |w: f64| if w > 0.0 { format!("{w} pt") } else { tl!("None").to_string() };
    let combo = craft_ui::ComboBox::from_id_salt("fill-text-border")
        .selected_text(border_label(was.width))
        .width(72.0)
        .show_ui(ui, |ui| {
            for w in crate::fill_sign::TEXT_BORDERS {
                ui.selectable_value(&mut border.width, w, border_label(w));
            }
        })
        .response;
    combo.on_hover_text(tl!("Border:"));
    // The border's own colour (the text's unless picked).
    if was.width > 0.0
        && let Some(color) = comments::border_colour_button(ui, was.color, [0.0; 3])
    {
        border.color = color;
    }
    // Its shape: an oval or a circle goes around the text, which keeps its place.
    if was.width > 0.0 {
        craft_ui::ComboBox::from_id_salt("fill-text-border-shape")
            .selected_text(tl!(was.shape.name()))
            .width(96.0)
            .show_ui(ui, |ui| {
                for shape in pdfcraft_engine::BorderShape::ALL {
                    ui.selectable_value(&mut border.shape, shape, tl!(shape.name()));
                }
            })
            .response
            .on_hover_text(tl!("Border shape"));
    }
    // No border, no shape: an oval or a circle left without its line would
    // keep its box grown around a shape that isn't drawn.
    if border.width <= 0.0 {
        border.shape = pdfcraft_engine::BorderShape::Rectangle;
    }
    match selected {
        Some((page, index, _)) => {
            // One undo step for what one choice changed (Border ▸ None takes a
            // circle's line and its shape).
            let mut edits = Vec::new();
            if border.width != was.width {
                edits.push(Edit::StyleAnnotation { page, index, color: None, opacity: None, width: Some(border.width), endings: None });
            }
            if border.color != was.color {
                edits.push(Edit::BorderColorAnnotation { page, index, color: border.color });
            }
            if border.shape != was.shape {
                edits.push(Edit::BorderShapeAnnotation { page, index, shape: border.shape });
            }
            if edits.len() > 1 {
                app.apply_edit(Edit::Batch { label: "Change comment properties".into(), edits });
            } else if let Some(edit) = edits.pop() {
                app.apply_edit(edit);
            }
        }
        None => {
            app.fill_text_border = border.width;
            app.fill_text_border_color = border.color;
            app.fill_text_border_shape = border.shape;
        }
    }
}

/// Extra controls rendered below/beside the buttons when a comment or fill tool is active.
fn active_tool_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let current_fill = match app.quick_tool {
        QuickTool::Fill(f) => Some(f),
        _ => None,
    };
    if matches!(current_fill, Some(crate::fill_sign::FillTool::Text | crate::fill_sign::FillTool::Date)) {
        controls_divider(ui, t);
        if let Some(color) = comments::quick_bar_colour(ui, app.fill_text_color) {
            app.fill_text_color = color;
            if let Some((page, index)) = selected_fill_text(app) {
                app.apply_edit(Edit::StyleAnnotation { page, index, color: Some(color), opacity: None, width: None, endings: None });
            }
        }
        let resp = icons::button(ui, "a-large-small", CELL, false, tl!("Font size"));
        let resized = egui::Popup::menu(&resp)
            .align(egui::RectAlign::BOTTOM_START)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(180.0);
                ui.label(egui::RichText::new(tl!("Font size")).font(theme::semibold(12.0)));
                let size = ui.add(
                    egui::Slider::new(&mut app.fill_text_size, crate::fill_sign::MIN_TEXT_SIZE..=crate::fill_sign::MAX_TEXT_SIZE)
                        .logarithmic(true)
                        .suffix(" pt"),
                );
                // #1274: the size also applies to the Fill & Sign text that is selected,
                // once per change (a typed value, or the end of a drag), so a drag adds
                // one undo step and not one per frame.
                (size.changed() && !size.dragged()) || size.drag_stopped()
            })
            .is_some_and(|popup| popup.inner);
        let size = app.fill_text_size;
        if resized
            && let Some((page, index)) = selected_fill_text(app)
            && app
                .active_ids()
                .and_then(|(_, id)| app.session.get(id)?.comment_props(page, index)?.font_size)
                .is_none_or(|stored| format!("{stored:.3}") != format!("{size:.3}"))
        {
            app.apply_edit(Edit::FontSizeAnnotation { page, index, size });
        }
        // After the text's own controls, so they keep their place in a bar that scrolls.
        fill_text_border_controls(ui, app, selected_fill_text(app));
    }
    if let QuickTool::Comment(tool) = app.quick_tool {
        controls_divider(ui, t);
        // Text boxes and callouts: the border's own colour, for new ones and the selected one
        // this tool made (not a locked one). With one selected the button shows its colours
        // (Comment Properties may have changed them), else the tool's.
        if tool.has_border_color() {
            let style = app.comment_prefs.style(tool);
            let selected = app.active_ids().and_then(|(index, id)| {
                let (page, annotation) = app.views.get(index)?.comments.selected?;
                let doc = app.session.get(id)?;
                let made = doc.allows_annotation()
                    && doc.info.annotations.iter().any(|a| a.page == page && a.index == annotation && !a.locked && comments::made_by(a, tool));
                made.then(|| (page, annotation, doc.comment_props(page, annotation)))
            });
            let (shown, text) = match selected.as_ref().and_then(|(.., p)| p.as_ref()) {
                Some(p) => (p.border_color, p.color.unwrap_or(style.color)),
                None => (style.border_color, style.color),
            };
            if let Some(color) = comments::border_colour_button(ui, shown, text) {
                app.comment_prefs.set_border_color(tool, color);
                if let Some((page, annotation, _)) = selected {
                    app.apply_edit(Edit::BorderColorAnnotation { page, index: annotation, color });
                }
            }
        }
        // The selected comments this tool made take the colour too (locked ones keep theirs).
        if let Some(color) = comments::quick_bar_controls(ui, tool, &mut app.comment_prefs)
            && let Some((_, id)) = app.active_ids()
            && app.session.get(id).is_some_and(|doc| doc.allows_annotation())
        {
            app.colour_selection(color, |a| comments::made_by(a, tool));
        }
    }
}
