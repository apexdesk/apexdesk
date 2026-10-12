//! Commands: what keys, menus and the command palette run.

use apex_doc::{Align, ParaKind};
use apex_io::Kind;
use apex_sheet::HAlign;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::Ask;

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Quit,
    QuitNow,
    SaveAndQuit,
    Save,
    SaveAs,
    ExportPdf,
    Palette,
    /// Opens the menu bar at its first menu.
    MenuBar,
    /// Shows or hides the menu bar (remembered).
    ToggleMenuBar,
    Help,
    Menu(Menu),
    Ask(Ask),
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    FindNext,
    SelectAll,
    Bold,
    Italic,
    Underline,
    Strike,
    // Sheets.
    EditCell,
    ClearContents,
    InsertRows,
    DeleteRows,
    InsertCols,
    DeleteCols,
    SortAsc,
    SortDesc,
    AutoFilter,
    FilterMenu,
    NumFormat(&'static str),
    Align(HAlign),
    TextColor(Option<u32>),
    FillColor(Option<u32>),
    ColWider,
    ColNarrower,
    AddSheet,
    DeleteSheet,
    NextSheet,
    PrevSheet,
    Recalc,
    Freeze,
    ShowFormulas,
    // Documents.
    ParaKind(ParaKind),
    ParaAlign(Align),
    Outline,
}

/// Submenus of the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Menu {
    NumFormat,
    Align,
    TextColor,
    FillColor,
    Insert,
    Delete,
    Sort,
    ParaStyle,
    ParaAlign,
}

/// Number formats of the format menu (label key, format code).
pub const NUM_FORMATS: [(&str, &str); 11] = [
    ("tui.fmt_general", "General"),
    ("tui.fmt_integer", "0"),
    ("tui.fmt_decimal", "0.00"),
    ("tui.fmt_thousands", "#,##0.00"),
    ("tui.fmt_percent", "0%"),
    ("tui.fmt_percent2", "0.00%"),
    ("tui.fmt_currency", "#,##0.00 €"),
    ("tui.fmt_date", "dd/mm/yyyy"),
    ("tui.fmt_time", "hh:mm"),
    ("tui.fmt_scientific", "0.00E+00"),
    ("tui.fmt_text", "@"),
];

/// Colors of the color menus (label key, 0xRRGGBB).
pub const COLORS: [(&str, u32); 10] = [
    ("tui.color_black", 0x000000),
    ("tui.color_white", 0xFFFFFF),
    ("tui.color_red", 0xC00000),
    ("tui.color_orange", 0xFFC000),
    ("tui.color_yellow", 0xFFFF00),
    ("tui.color_green", 0x00B050),
    ("tui.color_blue", 0x0070C0),
    ("tui.color_purple", 0x7030A0),
    ("tui.color_gray", 0x808080),
    ("tui.color_light_blue", 0xDDEBF7),
];

/// One entry of the palette: command, label key, key hint.
pub struct Entry {
    pub cmd: Cmd,
    pub label: &'static str,
    pub keys: &'static str,
}

pub(crate) const fn e(cmd: Cmd, label: &'static str, keys: &'static str) -> Entry {
    Entry { cmd, label, keys }
}

/// Commands of the palette for a kind of file.
pub fn palette(kind: Kind) -> Vec<Entry> {
    let mut v = vec![
        e(Cmd::Save, "tui.cmd_save", "Ctrl+S"),
        e(Cmd::SaveAs, "tui.cmd_save_as", "F12"),
        e(Cmd::ExportPdf, "tui.cmd_export_pdf", ""),
        e(Cmd::Undo, "tui.cmd_undo", "Ctrl+Z"),
        e(Cmd::Redo, "tui.cmd_redo", "Ctrl+Y"),
        e(Cmd::Copy, "tui.cmd_copy", "Ctrl+C"),
        e(Cmd::Cut, "tui.cmd_cut", "Ctrl+X"),
        e(Cmd::Paste, "tui.cmd_paste", "Ctrl+V"),
        e(Cmd::Ask(Ask::Find), "tui.cmd_find", "Ctrl+F"),
        e(Cmd::FindNext, "tui.cmd_find_next", "F3"),
        e(Cmd::Ask(Ask::ReplaceFind), "tui.cmd_replace", "Ctrl+H / Ctrl+R"),
        e(Cmd::SelectAll, "tui.cmd_select_all", "Ctrl+A"),
        e(Cmd::Bold, "tui.cmd_bold", "Ctrl+B"),
        e(Cmd::Italic, "tui.cmd_italic", "Ctrl+I / Alt+I"),
        e(Cmd::Underline, "tui.cmd_underline", "Ctrl+U"),
        e(Cmd::Strike, "tui.cmd_strike", "Alt+S"),
    ];
    match kind {
        Kind::Sheet => v.extend([
            e(Cmd::EditCell, "tui.cmd_edit_cell", "F2 / Enter"),
            e(Cmd::ClearContents, "tui.cmd_clear", "Delete"),
            e(Cmd::Ask(Ask::GoTo), "tui.cmd_goto", "Ctrl+G / F5"),
            e(Cmd::InsertRows, "tui.cmd_insert_rows", "Alt+R"),
            e(Cmd::DeleteRows, "tui.cmd_delete_rows", "Alt+Shift+R"),
            e(Cmd::InsertCols, "tui.cmd_insert_cols", "Alt+C"),
            e(Cmd::DeleteCols, "tui.cmd_delete_cols", "Alt+Shift+C"),
            e(Cmd::SortAsc, "tui.cmd_sort_asc", ""),
            e(Cmd::SortDesc, "tui.cmd_sort_desc", ""),
            e(Cmd::AutoFilter, "tui.cmd_autofilter", "Alt+L"),
            e(Cmd::FilterMenu, "tui.cmd_filter_menu", "Alt+Down"),
            e(Cmd::Menu(Menu::NumFormat), "tui.cmd_num_format", "Alt+N"),
            e(Cmd::Ask(Ask::NumFormat), "tui.cmd_custom_format", ""),
            e(Cmd::Menu(Menu::Align), "tui.cmd_align", "Alt+A"),
            e(Cmd::Menu(Menu::TextColor), "tui.cmd_text_color", ""),
            e(Cmd::Menu(Menu::FillColor), "tui.cmd_fill_color", ""),
            e(Cmd::ColWider, "tui.cmd_col_wider", "Alt+Right"),
            e(Cmd::ColNarrower, "tui.cmd_col_narrower", "Alt+Left"),
            e(Cmd::Ask(Ask::ColWidth), "tui.cmd_col_width", ""),
            e(Cmd::AddSheet, "tui.cmd_add_sheet", "Shift+F11"),
            e(Cmd::Ask(Ask::RenameSheet), "tui.cmd_rename_sheet", ""),
            e(Cmd::DeleteSheet, "tui.cmd_delete_sheet", ""),
            e(Cmd::NextSheet, "tui.cmd_next_sheet", "Ctrl+PgDn"),
            e(Cmd::PrevSheet, "tui.cmd_prev_sheet", "Ctrl+PgUp"),
            e(Cmd::Freeze, "tui.cmd_freeze", ""),
            e(Cmd::Recalc, "tui.cmd_recalc", "Shift+F9"),
            e(Cmd::ShowFormulas, "tui.cmd_show_formulas", "Ctrl+`"),
        ]),
        Kind::Doc | Kind::Slides => v.extend([
            e(Cmd::Menu(Menu::ParaStyle), "tui.cmd_para_style", "Alt+0..3"),
            e(Cmd::ParaKind(ParaKind::Normal), "tui.style_normal", "Alt+0"),
            e(Cmd::ParaKind(ParaKind::Heading(1)), "tui.style_h1", "Alt+1"),
            e(Cmd::ParaKind(ParaKind::Heading(2)), "tui.style_h2", "Alt+2"),
            e(Cmd::ParaKind(ParaKind::Heading(3)), "tui.style_h3", "Alt+3"),
            e(Cmd::ParaKind(ParaKind::Bullet), "tui.style_bullet", "Alt+B"),
            e(Cmd::ParaKind(ParaKind::Numbered), "tui.style_numbered", "Alt+N"),
            e(Cmd::Menu(Menu::ParaAlign), "tui.cmd_para_align", ""),
            e(Cmd::Outline, "tui.cmd_outline", "F6 / Alt+O"),
        ]),
    }
    v.extend([e(Cmd::MenuBar, "tui.cmd_open_menu", "F9"), e(Cmd::ToggleMenuBar, "tui.cmd_menu_bar", ""), e(Cmd::Help, "tui.cmd_help", "F1"), e(Cmd::Quit, "tui.cmd_quit", "Ctrl+Q")]);
    v
}

/// Entries of a submenu.
pub fn menu(m: Menu) -> (&'static str, Vec<Entry>) {
    match m {
        Menu::NumFormat => {
            let mut v: Vec<Entry> = NUM_FORMATS.iter().map(|&(k, f)| e(Cmd::NumFormat(f), k, f)).collect();
            v.push(e(Cmd::Ask(Ask::NumFormat), "tui.cmd_custom_format", ""));
            ("tui.cmd_num_format", v)
        }
        Menu::Align => (
            "tui.cmd_align",
            vec![
                e(Cmd::Align(HAlign::General), "tui.align_general", ""),
                e(Cmd::Align(HAlign::Left), "tui.align_left", ""),
                e(Cmd::Align(HAlign::Center), "tui.align_center", ""),
                e(Cmd::Align(HAlign::Right), "tui.align_right", ""),
            ],
        ),
        Menu::TextColor | Menu::FillColor => {
            let fg = m == Menu::TextColor;
            let mut v = vec![e(if fg { Cmd::TextColor(None) } else { Cmd::FillColor(None) }, if fg { "tui.color_auto" } else { "tui.color_none" }, "")];
            v.extend(COLORS.iter().map(|&(k, c)| e(if fg { Cmd::TextColor(Some(c)) } else { Cmd::FillColor(Some(c)) }, k, "")));
            v.push(e(Cmd::Ask(if fg { Ask::TextColor } else { Ask::FillColor }), "tui.color_custom", "#RRGGBB"));
            (if fg { "tui.cmd_text_color" } else { "tui.cmd_fill_color" }, v)
        }
        Menu::Insert => ("tui.cmd_insert", vec![e(Cmd::InsertRows, "tui.cmd_insert_rows", "Alt+R"), e(Cmd::InsertCols, "tui.cmd_insert_cols", "Alt+C")]),
        Menu::Delete => ("tui.cmd_delete", vec![e(Cmd::DeleteRows, "tui.cmd_delete_rows", "Alt+Shift+R"), e(Cmd::DeleteCols, "tui.cmd_delete_cols", "Alt+Shift+C")]),
        Menu::Sort => ("tui.cmd_sort", vec![e(Cmd::SortAsc, "tui.cmd_sort_asc", ""), e(Cmd::SortDesc, "tui.cmd_sort_desc", "")]),
        Menu::ParaStyle => (
            "tui.cmd_para_style",
            vec![
                e(Cmd::ParaKind(ParaKind::Normal), "tui.style_normal", "Alt+0"),
                e(Cmd::ParaKind(ParaKind::Title), "tui.style_title", ""),
                e(Cmd::ParaKind(ParaKind::Heading(1)), "tui.style_h1", "Alt+1"),
                e(Cmd::ParaKind(ParaKind::Heading(2)), "tui.style_h2", "Alt+2"),
                e(Cmd::ParaKind(ParaKind::Heading(3)), "tui.style_h3", "Alt+3"),
                e(Cmd::ParaKind(ParaKind::Bullet), "tui.style_bullet", "Alt+B"),
                e(Cmd::ParaKind(ParaKind::Numbered), "tui.style_numbered", "Alt+N"),
                e(Cmd::ParaKind(ParaKind::Quote), "tui.style_quote", ""),
                e(Cmd::ParaKind(ParaKind::Code), "tui.style_code", ""),
            ],
        ),
        Menu::ParaAlign => (
            "tui.cmd_para_align",
            vec![
                e(Cmd::ParaAlign(Align::Left), "tui.align_left", ""),
                e(Cmd::ParaAlign(Align::Center), "tui.align_center", ""),
                e(Cmd::ParaAlign(Align::Right), "tui.align_right", ""),
                e(Cmd::ParaAlign(Align::Justify), "tui.align_justify", ""),
            ],
        ),
    }
}

/// Keys that work the same everywhere (when no dialog is open).
pub fn global_key(k: &KeyEvent, kind: Kind, editing: bool) -> Option<Cmd> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    Some(match k.code {
        KeyCode::F(1) => Cmd::Help,
        KeyCode::F(9) if k.modifiers.is_empty() => Cmd::MenuBar,
        KeyCode::F(10) => Cmd::Palette,
        KeyCode::F(12) => Cmd::SaveAs,
        KeyCode::Char('p' | 'P') if ctrl => Cmd::Palette,
        KeyCode::Char('q' | 'Q') if ctrl => Cmd::Quit,
        KeyCode::Char('s' | 'S') if ctrl => Cmd::Save,
        KeyCode::Char('f' | 'F') if ctrl && !editing => Cmd::Ask(Ask::Find),
        KeyCode::Char('h' | 'H' | 'r' | 'R') if ctrl && !editing => Cmd::Ask(Ask::ReplaceFind),
        KeyCode::F(3) if !editing => Cmd::FindNext,
        KeyCode::Char('z' | 'Z') if ctrl && !editing => Cmd::Undo,
        KeyCode::Char('y' | 'Y') if ctrl && !editing => Cmd::Redo,
        KeyCode::Char('n' | 'N') if alt && kind == Kind::Sheet && !editing => Cmd::Menu(Menu::NumFormat),
        KeyCode::Char('a') if alt && kind == Kind::Sheet && !editing => Cmd::Menu(Menu::Align),
        _ => return None,
    })
}

/// Letters that Alt already uses as a shortcut (here and in the views'
/// keys): the menu bar's mnemonics leave them alone.
pub fn alt_taken(kind: Kind) -> &'static str {
    match kind {
        Kind::Sheet => "acilnrs",
        Kind::Doc | Kind::Slides => "bnios0123",
    }
}
