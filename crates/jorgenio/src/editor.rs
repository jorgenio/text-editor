//! Editor de texto: buffer + caret/anchor (selección) + historial + escena.

use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use jorgenio_core::buffer::{Buffer, LineEnding};
use jorgenio_core::history::{History, Transaction};
use jorgenio_core::movement;
use jorgenio_ui::gpu::srgb;
use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Cadena del parpadeo del caret (la usa el shell de la app con `WaitUntil`).
pub const CARET_BLINK: Duration = Duration::from_millis(530);

pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 20.0;
pub const TAB_WIDTH: usize = 4;

/// Inicio del área de código (tras el gutter).
const CODE_X: f32 = 64.0;
/// Derecha de los números de línea (alineados a la derecha).
const GUTTER_RIGHT_X: f32 = 56.0;
const PADDING_TOP: f32 = 8.0;
const CARET_WIDTH: f32 = 2.0;

const BG: (u8, u8, u8) = (0x27, 0x28, 0x22);
const FG: (u8, u8, u8) = (0xF8, 0xF8, 0xF2);
const GUTTER_FG: (u8, u8, u8) = (0x75, 0x71, 0x5E);
const GUTTER_CURRENT: (u8, u8, u8) = (0xFD, 0x97, 0x1F);
const CARET: (u8, u8, u8, u8) = (0xF8, 0xF8, 0xF2, 0xFF);
const SELECTION: (u8, u8, u8, u8) = (0x49, 0x48, 0x3E, 0xFF);

pub struct Editor {
    buffer: Buffer,
    history: History,
    path: Option<PathBuf>,
    /// Offset del caret (punto de inserción) en **chars**.
    caret: usize,
    /// Ancla de la selección; `anchor == caret` → sin selección.
    anchor: usize,
    /// Scroll vertical en px lógicos.
    scroll_y: f32,
    viewport: (f32, f32),
    focused: bool,
    caret_on: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            buffer: Buffer::new(),
            history: History::new(),
            path: None,
            caret: 0,
            anchor: 0,
            scroll_y: 0.0,
            viewport: (1200.0, 800.0),
            focused: true,
            caret_on: true,
        }
    }
}

impl Editor {
    /// Abre `path` si existe; si no, buffer nuevo con esa ruta (se crea al guardar).
    pub fn from_cli(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let buffer = match Buffer::load(&path) {
            Ok(buffer) => buffer,
            Err(err) => {
                log::warn!("no se pudo leer {}: {err}; se abrirá vacío", path.display());
                Buffer::new()
            }
        };
        Self {
            buffer,
            path: Some(path),
            ..Self::default()
        }
    }

    pub fn title(&self) -> String {
        let name = match &self.path {
            Some(path) => path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => "untitled".to_string(),
        };
        if self.buffer.dirty() {
            format!("\u{25cf} {name} \u{2014} Jorgenio Text")
        } else {
            format!("{name} \u{2014} Jorgenio Text")
        }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            log::warn!("guardar sin ruta: el diálogo llega en M9");
            return Ok(());
        };
        self.buffer.save(&path)
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.caret_on = true;
        }
    }

    pub fn toggle_caret(&mut self) {
        self.caret_on = !self.caret_on;
    }

    /// Atajo al escribir/moverse: el caret reaparece y se reinicia el blink.
    pub fn reset_caret_blink(&mut self) {
        self.caret_on = true;
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// Selección en rango normalizado `min..max` de chars.
    pub fn selected_range(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.buffer.len_chars();
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Teclado: insere, borra, mueve y selecciona con Shift. `true` = pide redraw.
    pub fn on_key(&mut self, event: &KeyEvent, modifiers: ModifiersState) -> bool {
        if event.state == ElementState::Released {
            return false;
        }
        // Los atajos (Ctrl/Cmd + tecla) los maneja el shell de la app.
        if (modifiers.control_key() || modifiers.super_key())
            && matches!(event.logical_key, Key::Character(_))
        {
            return false;
        }
        let shift = modifiers.shift_key();

        let handled = match &event.logical_key {
            Key::Named(NamedKey::Backspace) => {
                self.backspace();
                true
            }
            Key::Named(NamedKey::Delete) => {
                self.delete_forward();
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.insert_text("\n");
                true
            }
            Key::Named(NamedKey::Tab) => {
                self.insert_text("\t");
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                // Sin Shift, con selección: colapsa al borde izquierdo.
                let target = if !shift && self.has_selection() {
                    self.selected_range().start
                } else {
                    movement::prev_grapheme(&self.buffer, self.caret)
                };
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                // Sin Shift, con selección: colapsa al borde derecho.
                let target = if !shift && self.has_selection() {
                    self.selected_range().end
                } else {
                    movement::next_grapheme(&self.buffer, self.caret)
                };
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                let target = movement::vertical_move(&self.buffer, self.caret, -1, TAB_WIDTH);
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                let target = movement::vertical_move(&self.buffer, self.caret, 1, TAB_WIDTH);
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::Home) => {
                let target = movement::line_start(&self.buffer, self.caret);
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::End) => {
                let target = movement::line_end(&self.buffer, self.caret);
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::PageUp) => {
                let target = movement::vertical_move(
                    &self.buffer,
                    self.caret,
                    -self.page_lines(),
                    TAB_WIDTH,
                );
                self.move_caret(target, shift);
                true
            }
            Key::Named(NamedKey::PageDown) => {
                let target =
                    movement::vertical_move(&self.buffer, self.caret, self.page_lines(), TAB_WIDTH);
                self.move_caret(target, shift);
                true
            }
            Key::Character(text) => {
                self.insert_text(text);
                true
            }
            _ => false,
        };
        if handled {
            self.reset_caret_blink();
        }
        handled
    }

    /// Inserta texto (teclado, IME o pegado): reemplaza la selección como UNA
    /// transacción de undo. Normaliza `\r\n`/`\r` a `\n`.
    pub fn insert_text(&mut self, text: &str) {
        let cleaned: Cow<'_, str> = if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        };
        if cleaned.is_empty() {
            return;
        }

        let before = (self.caret, self.anchor);
        let range = self.selected_range();
        self.buffer.begin();
        if !range.is_empty() {
            self.buffer.delete(range.clone());
            self.caret = range.start;
        }
        self.buffer.insert(self.caret, &cleaned);
        self.caret += cleaned.chars().count();
        self.anchor = self.caret;
        self.commit(before);
        self.scroll_into_view();
    }

    pub fn backspace(&mut self) {
        if self.has_selection() {
            let range = self.selected_range();
            self.delete_range(range);
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = movement::prev_grapheme(&self.buffer, self.caret);
        self.delete_range(start..self.caret);
    }

    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            let range = self.selected_range();
            self.delete_range(range);
            return;
        }
        if self.caret >= self.buffer.len_chars() {
            return;
        }
        let end = movement::next_grapheme(&self.buffer, self.caret);
        self.delete_range(self.caret..end);
    }

    /// Texto de la selección con el EOL del archivo (para el portapapeles).
    pub fn copy(&self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        let text = self.buffer.text_range(self.selected_range());
        Some(match self.buffer.line_ending() {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        })
    }

    /// Copia la selección y la borra (una transacción de undo).
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        let range = self.selected_range();
        self.delete_range(range);
        Some(text)
    }

    pub fn undo(&mut self) -> bool {
        if let Some((caret, anchor)) = self.history.undo(&mut self.buffer) {
            self.caret = caret;
            self.anchor = anchor;
            self.scroll_into_view();
            self.reset_caret_blink();
            return true;
        }
        false
    }

    pub fn redo(&mut self) -> bool {
        if let Some((caret, anchor)) = self.history.redo(&mut self.buffer) {
            self.caret = caret;
            self.anchor = anchor;
            self.scroll_into_view();
            self.reset_caret_blink();
            return true;
        }
        false
    }

    /// Click: `count`=1 caret (o extiende con Shift), 2 palabra, ≥3 línea.
    pub fn on_click(&mut self, x: f32, y: f32, advance: f32, count: u32, shift: bool) {
        self.reset_caret_blink();
        if advance <= 0.0 {
            return;
        }
        let pos = self.caret_at(x, y, advance);
        match (count, shift) {
            // Shift+click: extiende la selección desde el ancla.
            (1, true) => self.caret = pos,
            (2, _) => {
                let range = movement::word_range(&self.buffer, pos);
                self.anchor = range.start;
                self.caret = range.end;
            }
            (3.., _) => {
                let line = self.buffer.char_to_line(pos);
                let range = movement::line_range(&self.buffer, line);
                self.anchor = range.start;
                self.caret = range.end;
            }
            _ => {
                self.caret = pos;
                self.anchor = pos;
            }
        }
        self.scroll_into_view();
    }

    /// Extiende la selección hasta el cursor (arrastre con el botón izquierdo).
    pub fn select_to(&mut self, x: f32, y: f32, advance: f32) {
        if advance <= 0.0 {
            return;
        }
        self.caret = self.caret_at(x, y, advance);
        self.scroll_into_view();
    }

    /// Rueda: `delta_y` en px lógicos positivos hacia arriba.
    pub fn on_wheel(&mut self, delta_y: f32) {
        self.scroll_y -= delta_y;
        self.clamp_scroll();
    }

    /// Escena de este frame: selección + gutter + líneas visibles + caret.
    pub fn build_scene(&self, viewport: (f32, f32), advance_ratio: f32) -> Scene {
        let mut scene = Scene::new(srgb(BG.0, BG.1, BG.2));
        let advance = FONT_SIZE * advance_ratio;
        let line_count = self.buffer.line_count();

        let visible_first = (((self.scroll_y - PADDING_TOP) / LINE_HEIGHT)
            .floor()
            .max(0.0) as usize)
            .min(line_count - 1);
        let visible_last = (((self.scroll_y + viewport.1 - PADDING_TOP) / LINE_HEIGHT).ceil()
            as usize)
            .min(line_count - 1);

        if self.has_selection() {
            let range = self.selected_range();
            let first = self.buffer.char_to_line(range.start);
            let last = self.buffer.char_to_line(range.end.saturating_sub(1));
            for line in first..=last {
                if line < visible_first || line > visible_last {
                    continue;
                }
                let y = PADDING_TOP + line as f32 * LINE_HEIGHT - self.scroll_y;
                let text = self.buffer.line_text(line);
                let line_start = self.buffer.line_to_char(line);

                let start_cells = if line == first {
                    let col = range.start - line_start;
                    movement::visual_column(&text, col, TAB_WIDTH)
                } else {
                    0
                };
                let end_cells = if line == last {
                    let col = range.end - line_start;
                    // Seleccionar el \n añade una celda más (como Sublime).
                    if col > text.chars().count() {
                        movement::visual_column(&text, text.chars().count(), TAB_WIDTH) + 1
                    } else {
                        movement::visual_column(&text, col, TAB_WIDTH)
                    }
                } else {
                    movement::visual_column(&text, text.chars().count(), TAB_WIDTH)
                };

                let x = CODE_X + start_cells as f32 * advance;
                let w = (end_cells.saturating_sub(start_cells)) as f32 * advance;
                if w > 0.0 {
                    scene.quads.push(Quad::new(x, y, w, LINE_HEIGHT, SELECTION));
                }
            }
        }

        let caret_line = self.buffer.char_to_line(self.caret);
        for line in visible_first..=visible_last {
            let y = PADDING_TOP + line as f32 * LINE_HEIGHT - self.scroll_y;
            let color = if line == caret_line {
                GUTTER_CURRENT
            } else {
                GUTTER_FG
            };
            let number = format!("{}", line + 1);
            let number_x = GUTTER_RIGHT_X - number.len() as f32 * advance;
            scene
                .texts
                .push(TextSpan::new(number, number_x, y, FONT_SIZE, color));

            let text = self.buffer.line_text(line);
            if !text.is_empty() {
                scene
                    .texts
                    .push(TextSpan::new(text, CODE_X, y, FONT_SIZE, FG));
            }
        }

        if self.focused && self.caret_on {
            let y = PADDING_TOP + caret_line as f32 * LINE_HEIGHT - self.scroll_y;
            let col = self.caret - self.buffer.line_to_char(caret_line);
            let cells = movement::visual_column(&self.buffer.line_text(caret_line), col, TAB_WIDTH);
            scene.overlays.push(Quad::new(
                CODE_X + cells as f32 * advance,
                y,
                CARET_WIDTH,
                LINE_HEIGHT - 2.0,
                CARET,
            ));
        }
        scene
    }

    /// Borra un rango como transacción y colapsa la selección ahí.
    fn delete_range(&mut self, range: Range<usize>) {
        let before = (self.caret, self.anchor);
        self.buffer.begin();
        self.buffer.delete(range.clone());
        self.caret = range.start;
        self.anchor = range.start;
        self.commit(before);
        self.scroll_into_view();
    }

    /// Cierra la transacción abierta con `before` y registra los carets actuales.
    fn commit(&mut self, before: (usize, usize)) {
        let edits = self.buffer.take_edits();
        let after = (self.caret, self.anchor);
        self.history.commit(Transaction::new(edits, before, after));
    }

    fn move_caret(&mut self, caret: usize, shift: bool) {
        self.caret = caret;
        if !shift {
            self.anchor = caret;
        }
        self.scroll_into_view();
    }

    /// Coordenadas lógicas → offset de char (columna visual, media celda).
    fn caret_at(&self, x: f32, y: f32, advance: f32) -> usize {
        let visual = ((x - CODE_X) / advance).floor().max(0.0) as usize;
        let line_f = ((y - PADDING_TOP + self.scroll_y) / LINE_HEIGHT).floor();
        let line = (line_f.max(0.0) as usize).min(self.buffer.line_count() - 1);
        let text = self.buffer.line_text(line);
        let char_col = movement::char_col_at_visual(&text, visual, TAB_WIDTH);
        self.buffer.line_to_char(line) + char_col
    }

    fn page_lines(&self) -> isize {
        ((self.viewport.1 / LINE_HEIGHT) as isize).max(1)
    }

    fn scroll_into_view(&mut self) {
        let line = self.buffer.char_to_line(self.caret);
        let top = PADDING_TOP + line as f32 * LINE_HEIGHT;
        let bottom = top + LINE_HEIGHT;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + self.viewport.1 {
            self.scroll_y = bottom - self.viewport.1;
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let content_h = PADDING_TOP + self.buffer.line_count() as f32 * LINE_HEIGHT + PADDING_TOP;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - self.viewport.1).max(0.0));
    }
}

/// Ruta del archivo a abrir, si la pasaron por CLI (M9: args completos).
pub fn cli_path() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_joins_into_one_undo_step() {
        let mut editor = Editor::default();
        for ch in "hola".chars() {
            editor.insert_text(&ch.to_string());
        }
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            "hola"
        );

        assert!(editor.undo());
        assert_eq!(editor.buffer.text_range(0..editor.buffer.len_chars()), "");
        assert!(editor.redo());
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            "hola"
        );
        assert_eq!(editor.caret, 4);
    }

    #[test]
    fn replace_selection_is_one_atomic_undo() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        editor.caret = 0;
        editor.anchor = 4; // selecciona "hola"
        editor.insert_text("x");
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            "x mundo"
        );

        assert!(editor.undo());
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            "hola mundo"
        );
        assert_eq!((editor.caret, editor.anchor), (0, 4), "restaura carets");
    }

    #[test]
    fn backspace_after_undo_redo_run() {
        let mut editor = Editor::default();
        editor.insert_text("abc");
        editor.backspace();
        editor.backspace();
        assert_eq!(editor.buffer.text_range(0..editor.buffer.len_chars()), "a");
        assert!(editor.undo(), "los backspaces se deshacen juntos");
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            "abc"
        );
    }

    #[test]
    fn copy_cut_and_paste_roundtrip() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        editor.caret = 0;
        editor.anchor = 4;

        assert_eq!(editor.copy().as_deref(), Some("hola"));
        let cut = editor.cut().expect("cut debe devolver el texto");
        assert_eq!(cut, "hola");
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            " mundo"
        );

        editor.caret = editor.buffer.len_chars();
        editor.anchor = editor.caret;
        editor.insert_text(&cut);
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            " mundohola"
        );
        assert!(editor.undo(), "el pegado es un solo undo");
        assert_eq!(
            editor.buffer.text_range(0..editor.buffer.len_chars()),
            " mundo"
        );
    }

    #[test]
    fn double_and_triple_click_select_word_and_line() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        let advance = 10.0;

        // Doble click sobre "hola" (x=0) → palabra.
        editor.on_click(CODE_X, 0.0, advance, 2, false);
        assert_eq!(editor.selected_range(), 0..4);

        // Triple click → línea completa (con \n si existe; aquí no lo hay).
        editor.on_click(CODE_X, 0.0, advance, 3, false);
        assert_eq!(editor.selected_range(), 0..10);

        // Con fin de línea, la selección incluye el \n.
        let mut editor = Editor::default();
        editor.insert_text("hola\nmundo");
        editor.on_click(CODE_X, 0.0, advance, 3, false);
        assert_eq!(editor.selected_range(), 0..5);
    }

    #[test]
    fn shift_arrow_extends_and_plain_arrow_collapses() {
        let mut editor = Editor::default();
        editor.insert_text("abc");
        editor.caret = 0;
        editor.anchor = 0;

        // Shift+Right extiende.
        editor.move_caret(1, true);
        assert_eq!(editor.selected_range(), 0..1);
        editor.move_caret(2, true);
        assert_eq!(editor.selected_range(), 0..2);

        // Sin Shift, colapsa al borde izquierdo.
        editor.caret = editor.selected_range().start;
        editor.anchor = editor.caret;
        assert!(!editor.has_selection());
    }
}
