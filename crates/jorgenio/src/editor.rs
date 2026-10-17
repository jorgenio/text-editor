//! Editor de texto: buffer + cursores (multi-cursor) + historial + escena.
//!
//! El estado de entrada vive en `cursors` (siempre ≥ 1). El **primario** es
//! `cursors[0]` (el de más a la izquierda tras [`Editor::sort_and_merge`]).
//! Las ediciones se aplican de derecha a izquierda (los edits altos no afectan
//! a las posiciones bajas aún sin procesar) y se registran como UNA
//! transacción de undo con los carets antes/después.

use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;

use jorgenio_core::buffer::{Buffer, LineEnding};
use jorgenio_core::history::{History, Transaction};
use jorgenio_core::movement;
use jorgenio_syntax::SyntaxHighlighter;
use jorgenio_ui::gpu::srgb;
use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::snippets;
use crate::theme::Theme;

pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 20.0;
/// Ancho de tabulador por defecto (el `settings.json` lo sobrescribe).
pub const TAB_WIDTH: usize = 4;

/// Inicio del área de código (tras el gutter).
const CODE_X: f32 = 64.0;
/// Inicio del código con el gutter oculto (`show_line_numbers: false`).
const HIDDEN_GUTTER_X: f32 = 12.0;
/// Derecha de los números de línea (alineados a la derecha).
const GUTTER_RIGHT_X: f32 = 56.0;
const PADDING_TOP: f32 = 8.0;
const CARET_WIDTH: f32 = 2.0;

/// Un cursor con su selección (`anchor == caret` → sin selección).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Offset del punto de inserción en **chars**.
    pub caret: usize,
    /// Ancla de la selección.
    pub anchor: usize,
}

impl Cursor {
    pub fn new(caret: usize) -> Self {
        Self {
            caret,
            anchor: caret,
        }
    }

    /// Selección en rango normalizado `min..max` de chars.
    pub fn range(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }
}

pub struct Editor {
    buffer: Buffer,
    history: History,
    /// Resaltado por gramática (syntect) con cache incremental.
    syntax: SyntaxHighlighter,
    /// Extensión del archivo ("rs", "py"…); "" = texto plano.
    lang: String,
    path: Option<PathBuf>,
    /// Cursores con su selección; ordenados y sin solapes (invariante).
    cursors: Vec<Cursor>,
    /// Scroll vertical en px lógicos.
    scroll_y: f32,
    viewport: (f32, f32),
    focused: bool,
    caret_on: bool,
    /// Hueco superior (px) reservado a la barra de pestañas: todo el dibujo
    /// y toda la geometría (clicks, scroll, visibles) se desplaza con él.
    top_inset: f32,
    /// Hueco izquierdo (px): desplaza el gutter y el código para que el panel
    /// derecho de un split no pinte encima del izquierdo.
    left_inset: f32,
    /// Ancho de tabulador en celdas (preferencias; ver [`TAB_WIDTH`]).
    tab_width: usize,
    /// ¿Dibujar los números de línea? (`show_line_numbers` de preferencias).
    show_line_numbers: bool,
    /// Tema de colores activo (oscuro Monokai / claro); lo fija `main`.
    theme: Theme,
    /// Buscar (Ctrl+F): rangos de `find_query` en orden, y el actual.
    find: Vec<Range<usize>>,
    find_current: usize,
    find_query: String,
    /// Revisión del buffer con la que `find` está calculado.
    find_revision: u64,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            buffer: Buffer::new(),
            history: History::new(),
            syntax: SyntaxHighlighter::new(),
            lang: String::new(),
            path: None,
            cursors: vec![Cursor::new(0)],
            scroll_y: 0.0,
            viewport: (1200.0, 800.0),
            focused: true,
            caret_on: true,
            top_inset: 0.0,
            left_inset: 0.0,
            tab_width: TAB_WIDTH,
            show_line_numbers: true,
            theme: Theme::dark(),
            find: Vec::new(),
            find_current: 0,
            find_query: String::new(),
            find_revision: 0,
        }
    }
}

impl Editor {
    /// Abre `path` si existe; si no, buffer nuevo con esa ruta (se crea al guardar).
    pub fn from_cli(path: PathBuf) -> Self {
        let lang = SyntaxHighlighter::detect(&path);
        let buffer = match Buffer::load(&path) {
            Ok(buffer) => buffer,
            Err(err) => {
                log::warn!("no se pudo leer {}: {err}; se abrirá vacío", path.display());
                Buffer::new()
            }
        };
        Self {
            buffer,
            lang,
            path: Some(path),
            ..Self::default()
        }
    }

    /// Título de la ventana: nombre del archivo + indicador de cambios.
    pub fn title(&self) -> String {
        let name = self.tab_label();
        if self.buffer.dirty() {
            format!("\u{25cf} {name} \u{2014} Jorgenio Text")
        } else {
            format!("{name} \u{2014} Jorgenio Text")
        }
    }

    /// Nombre corto para la barra de pestañas (sin la ruta completa).
    pub fn tab_label(&self) -> String {
        match &self.path {
            Some(path) => path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => "sin título".to_string(),
        }
    }

    /// ¿Tiene ruta fija? (sin ella Ctrl+S abre el diálogo de guardar).
    pub fn has_path(&self) -> bool {
        self.path.is_some()
    }

    /// Ruta fija del archivo, si la tiene (para detectar pestañas duplicadas).
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// ¿Hay cambios sin guardar? (el "●" de la pestaña y del título).
    pub fn dirty(&self) -> bool {
        self.buffer.dirty()
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            log::warn!("guardar sin ruta: usa save_as con el diálogo");
            return Ok(());
        };
        self.buffer.save(&path)
    }

    /// Guarda en una ruta nueva (lo decide el diálogo de "Guardar como…"):
    /// fija la ruta, recalcula la resaltación por extensión y escribe.
    pub fn save_as(&mut self, path: PathBuf) -> std::io::Result<()> {
        self.lang = SyntaxHighlighter::detect(&path);
        self.path = Some(path.clone());
        self.buffer.save(&path)
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
    }

    /// Reserva los `px` superiores para la barra de pestañas (0 = sin barra).
    pub fn set_top_inset(&mut self, px: f32) {
        self.top_inset = px;
    }

    /// Desplaza horizontalmente todo el panel (0 = pegado al borde izquierdo).
    pub fn set_left_inset(&mut self, px: f32) {
        self.left_inset = px;
    }

    /// Ancho de tabulador en celdas (recortado a 1-32: 0 partiría la
    /// aritmética de columnas). El clic y el caret se recalculan solos.
    pub fn set_tab_width(&mut self, width: usize) {
        self.tab_width = width.clamp(1, 32);
    }

    pub fn set_show_line_numbers(&mut self, show: bool) {
        self.show_line_numbers = show;
    }

    /// Fija el tema de colores; si cambia la paleta de syntect se invalida
    /// el resaltado cacheado y el siguiente frame re-resalta en el nuevo tono.
    pub fn set_theme(&mut self, theme: Theme) {
        if theme.syntax != self.theme.syntax {
            self.syntax.set_theme(theme.syntax);
        }
        self.theme = theme;
    }

    /// Solo para tests (comprobar `apply_prefs` y `theme.toggle`).
    #[cfg(test)]
    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }

    /// Inicio del área de código según el gutter (preferencias).
    fn code_offset(&self) -> f32 {
        if self.show_line_numbers {
            CODE_X
        } else {
            HIDDEN_GUTTER_X
        }
    }

    /// Solo para tests (comprobar la geometría de un split).
    #[cfg(test)]
    pub fn left_inset(&self) -> f32 {
        self.left_inset
    }

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn show_line_numbers(&self) -> bool {
        self.show_line_numbers
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

    /// Selección completa del buffer (un solo cursor primario).
    pub fn select_all(&mut self) {
        self.cursors = vec![Cursor {
            caret: self.buffer.len_chars(),
            anchor: 0,
        }];
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Teclado: insere, borra, mueve y selecciona con Shift. `true` = pide redraw.
    pub fn on_key(&mut self, event: &KeyEvent, modifiers: ModifiersState) -> bool {
        if event.state == ElementState::Released {
            return false;
        }
        self.handle_key(&event.logical_key, modifiers)
    }

    /// Núcleo de [`Editor::on_key`] (separable para los tests, ya que
    /// `KeyEvent` no se puede construir fuera de winit).
    fn handle_key(&mut self, key: &Key, modifiers: ModifiersState) -> bool {
        // Los atajos (Ctrl/Cmd + tecla) los maneja el shell de la app.
        // Con AltGr (Ctrl+Alt en Windows) NO: esa combinación escribe
        // símbolos del layout (@, €, {…}), que deben insertarse.
        if (modifiers.control_key() || modifiers.super_key())
            && !modifiers.alt_key()
            && matches!(key, Key::Character(_))
        {
            return false;
        }
        let shift = modifiers.shift_key();

        let handled = match key {
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
                if self.try_expand_snippet() {
                    true
                } else {
                    self.insert_text("\t");
                    true
                }
            }
            Key::Named(NamedKey::Escape) => {
                self.collapse_to_primary();
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                // Sin Shift, con selección: colapsa al borde izquierdo.
                let collapse = !shift;
                self.move_cursors(
                    |cursor, buffer| {
                        if collapse && cursor.has_selection() {
                            cursor.range().start
                        } else {
                            movement::prev_grapheme(buffer, cursor.caret)
                        }
                    },
                    shift,
                );
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                // Sin Shift, con selección: colapsa al borde derecho.
                let collapse = !shift;
                self.move_cursors(
                    |cursor, buffer| {
                        if collapse && cursor.has_selection() {
                            cursor.range().end
                        } else {
                            movement::next_grapheme(buffer, cursor.caret)
                        }
                    },
                    shift,
                );
                true
            }
            Key::Named(NamedKey::ArrowUp) if modifiers.alt_key() && modifiers.control_key() => {
                self.add_cursor_vertical(-1);
                true
            }
            Key::Named(NamedKey::ArrowDown) if modifiers.alt_key() && modifiers.control_key() => {
                self.add_cursor_vertical(1);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                let tab_width = self.tab_width;
                self.move_cursors(
                    |cursor, buffer| movement::vertical_move(buffer, cursor.caret, -1, tab_width),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                let tab_width = self.tab_width;
                self.move_cursors(
                    |cursor, buffer| movement::vertical_move(buffer, cursor.caret, 1, tab_width),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::Home) => {
                self.move_cursors(
                    |cursor, buffer| movement::line_start(buffer, cursor.caret),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::End) => {
                self.move_cursors(
                    |cursor, buffer| movement::line_end(buffer, cursor.caret),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::PageUp) => {
                let page = -self.page_lines();
                let tab_width = self.tab_width;
                self.move_cursors(
                    |cursor, buffer| movement::vertical_move(buffer, cursor.caret, page, tab_width),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::PageDown) => {
                let page = self.page_lines();
                let tab_width = self.tab_width;
                self.move_cursors(
                    |cursor, buffer| movement::vertical_move(buffer, cursor.caret, page, tab_width),
                    shift,
                );
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

    /// Inserta texto en TODOS los cursores (teclado, IME o pegado):
    /// reemplaza cada selección y deja UNA transacción de undo.
    /// Normaliza `\r\n`/`\r` a `\n`.
    pub fn insert_text(&mut self, text: &str) {
        let cleaned: Cow<'_, str> = if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        };
        if cleaned.is_empty() {
            return;
        }
        let len = cleaned.chars().count();

        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let range = editor.cursors[i].range();
                let (start, end) = (range.start, range.end);
                if end > start {
                    editor.buffer.delete(start..end);
                }
                editor.buffer.insert(start, &cleaned);
                let pos = start + len;
                // Los cursores de la derecha ya están en su sitio: solo se
                // desplazan por este edit.
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, start, end, len);
                    other.anchor = shift_after(other.anchor, start, end, len);
                }
                editor.cursors[i].caret = pos;
                editor.cursors[i].anchor = pos;
            }
        });
        self.reset_caret_blink();
    }

    pub fn backspace(&mut self) {
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let range = editor.cursors[i].range();
                let (start, end) = if !range.is_empty() {
                    (range.start, range.end)
                } else if range.start > 0 {
                    (
                        movement::prev_grapheme(&editor.buffer, range.start),
                        range.start,
                    )
                } else {
                    continue; // sin selección y al inicio del buffer
                };
                editor.buffer.delete(start..end);
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, start, end, 0);
                    other.anchor = shift_after(other.anchor, start, end, 0);
                }
                editor.cursors[i].caret = start;
                editor.cursors[i].anchor = start;
            }
        });
        self.reset_caret_blink();
    }

    pub fn delete_forward(&mut self) {
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let range = editor.cursors[i].range();
                let (start, end) = if !range.is_empty() {
                    (range.start, range.end)
                } else if range.start < editor.buffer.len_chars() {
                    (
                        range.start,
                        movement::next_grapheme(&editor.buffer, range.start),
                    )
                } else {
                    continue; // sin selección y al final del buffer
                };
                editor.buffer.delete(start..end);
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, start, end, 0);
                    other.anchor = shift_after(other.anchor, start, end, 0);
                }
                editor.cursors[i].caret = start;
                editor.cursors[i].anchor = start;
            }
        });
        self.reset_caret_blink();
    }

    /// Texto de TODAS las selecciones unidas con `\n` (portapapeles).
    pub fn copy(&self) -> Option<String> {
        let parts: Vec<String> = self
            .cursors
            .iter()
            .filter(|cursor| cursor.has_selection())
            .map(|cursor| self.buffer.text_range(cursor.range()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        let text = parts.join("\n");
        Some(match self.buffer.line_ending() {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        })
    }

    /// Copia las selecciones y las borra (una transacción de undo).
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let range = editor.cursors[i].range();
                if range.is_empty() {
                    continue; // los cursores sin selección solo se desplazan
                }
                editor.buffer.delete(range.clone());
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, range.start, range.end, 0);
                    other.anchor = shift_after(other.anchor, range.start, range.end, 0);
                }
                editor.cursors[i].caret = range.start;
                editor.cursors[i].anchor = range.start;
            }
        });
        Some(text)
    }

    /// Líneas (0-based) que contienen cursores, ordenadas y sin duplicados.
    fn cursor_lines(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self
            .cursors
            .iter()
            .map(|cursor| self.buffer.char_to_line(cursor.caret))
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// Borra TODAS las líneas con cursores (Ctrl+Shift+K): una sola transacción.
    pub fn delete_current_line(&mut self) {
        let lines = self.cursor_lines();
        if lines.is_empty() {
            return;
        }
        self.with_transaction(|editor| {
            // Descendente: borrar líneas altas no desplaza las de abajo.
            let mut i = lines.len();
            while i > 0 {
                i -= 1;
                let line = lines[i];
                let (start, end) = if line + 1 < editor.buffer.line_count() {
                    // Incluye el \n final de la línea.
                    (
                        editor.buffer.line_to_char(line),
                        editor.buffer.line_to_char(line + 1),
                    )
                } else {
                    let start = editor.buffer.line_to_char(line);
                    let len = editor.buffer.len_chars();
                    if start >= len {
                        // Última línea vacía ("a\n" con caret en línea 1):
                        // solo existe el \n anterior que la separa.
                        (start.saturating_sub(1), start)
                    } else {
                        // Sin \n final: se lleva solo el contenido.
                        (start, len)
                    }
                };
                if start >= end {
                    continue;
                }
                editor.buffer.delete(start..end);
                for cursor in &mut editor.cursors {
                    cursor.caret = shift_after(cursor.caret, start, end, 0);
                    cursor.anchor = shift_after(cursor.anchor, start, end, 0);
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Duplica hacia abajo cada línea con cursor (Ctrl+Shift+D): una
    /// transacción; los cursores quedan en las líneas originales.
    pub fn duplicate_current_line(&mut self) {
        let lines = self.cursor_lines();
        if lines.is_empty() {
            return;
        }
        self.with_transaction(|editor| {
            let mut i = lines.len();
            while i > 0 {
                i -= 1;
                let line = lines[i];
                let last = line + 1 >= editor.buffer.line_count();
                let content = editor.buffer.line_text(line);
                // La última línea no tiene \n: se copia añadiéndolo delante.
                let payload = if last {
                    format!("\n{content}")
                } else {
                    format!("{content}\n")
                };
                let at = if last {
                    editor.buffer.len_chars()
                } else {
                    editor.buffer.line_to_char(line + 1)
                };
                let len = payload.chars().count();
                editor.buffer.insert(at, &payload);
                for cursor in &mut editor.cursors {
                    cursor.caret = shift_after(cursor.caret, at, at, len);
                    cursor.anchor = shift_after(cursor.anchor, at, at, len);
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Comenta o desenvuelve las líneas de todos los cursores (Ctrl+/). Si
    /// TODAS las líneas con contenido están comentadas las desenvuelve; si no,
    /// añade el prefijo solo a las que falten (nunca duplica comentarios).
    /// Las líneas vacías también reciben el prefijo al comentar.
    pub fn toggle_line_comment(&mut self) {
        let prefix = comment_prefix(&self.lang);
        let lines = self.comment_lines();
        if lines.is_empty() {
            return;
        }

        let mut any_content = false;
        let mut all_commented = true;
        for &line in &lines {
            let text = self.buffer.line_text(line);
            let body = trim_line_ending(&text);
            if body.trim().is_empty() {
                continue;
            }
            any_content = true;
            if !body.trim_start().starts_with(prefix) {
                all_commented = false;
                break;
            }
        }
        let commenting = !(any_content && all_commented);

        self.with_transaction(|editor| {
            // Descendente: cada línea se edita en sus coordenadas originales.
            for &line in lines.iter().rev() {
                let line_start = editor.buffer.line_to_char(line);
                let body = trim_line_ending(&editor.buffer.line_text(line)).to_string();
                let trimmed = body.trim_start();
                let ws = body.chars().count() - trimmed.chars().count();
                if commenting {
                    if trimmed.starts_with(prefix) {
                        continue; // ya comentada: no duplica
                    }
                    let at = line_start + ws;
                    let payload = format!("{prefix} ");
                    editor.buffer.insert(at, &payload);
                    let len = payload.chars().count();
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, at, at, len);
                        cursor.anchor = shift_after(cursor.anchor, at, at, len);
                    }
                } else if let Some(after) = trimmed.strip_prefix(prefix) {
                    let mut remove = prefix.chars().count();
                    if after.starts_with(' ') {
                        remove += 1;
                    }
                    let start = line_start + ws;
                    let end = start + remove;
                    editor.buffer.delete(start..end);
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, start, end, 0);
                        cursor.anchor = shift_after(cursor.anchor, start, end, 0);
                    }
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Envuelve la selección (o la línea actual, sin selección) en
    /// `/* … */`; si ya está envuelta, desenvuelve los marcadores.
    pub fn toggle_block_comment(&mut self) {
        let range = self.primary().range();
        let (start, end) = if range.is_empty() {
            let line = self.buffer.char_to_line(range.start);
            let line_start = self.buffer.line_to_char(line);
            let body = trim_line_ending(&self.buffer.line_text(line)).to_string();
            (line_start, line_start + body.chars().count())
        } else {
            (range.start, range.end)
        };
        if start >= end {
            return; // línea vacía y sin selección
        }
        let text = self.buffer.text_range(start..end);

        // ¿Hay un comentario de bloque que desenvolver? (sin solapamientos raros)
        let mut unwrap = None;
        let trimmed = text.trim();
        if trimmed.starts_with("/*")
            && trimmed.ends_with("*/")
            && trimmed.chars().count() >= 4
            && let (Some(open_b), Some(close_b)) = (text.find("/*"), text.rfind("*/"))
            && close_b >= open_b + 2
        {
            let open_at = start + text[..open_b].chars().count();
            let close_at = start + text[..close_b].chars().count();
            let open_to = open_at
                + if text[open_b..].starts_with("/* ") {
                    3
                } else {
                    2
                };
            let close_from = if close_b > 0 && text.as_bytes()[close_b - 1] == b' ' {
                close_at - 1
            } else {
                close_at
            };
            unwrap =
                (open_to <= close_from).then_some((open_at, open_to, close_from, close_at + 2));
        }

        self.with_transaction(|editor| {
            match unwrap {
                Some((open_at, open_to, close_from, close_to)) => {
                    // Descendente: primero el cierre, después la apertura.
                    editor.buffer.delete(close_from..close_to);
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, close_from, close_to, 0);
                        cursor.anchor = shift_after(cursor.anchor, close_from, close_to, 0);
                    }
                    editor.buffer.delete(open_at..open_to);
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, open_at, open_to, 0);
                        cursor.anchor = shift_after(cursor.anchor, open_at, open_to, 0);
                    }
                }
                None => {
                    // Envuelve: primero el cierre (coordenadas originales)
                    // y luego la apertura.
                    editor.buffer.insert(end, " */");
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, end, end, 3);
                        cursor.anchor = shift_after(cursor.anchor, end, end, 3);
                    }
                    editor.buffer.insert(start, "/* ");
                    for cursor in &mut editor.cursors {
                        cursor.caret = shift_after(cursor.caret, start, start, 3);
                        cursor.anchor = shift_after(cursor.anchor, start, start, 3);
                    }
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Tab: si justo antes del caret hay una palabra-clave con snippet para
    /// esta extensión, la expande en una sola transacción de undo y devuelve
    /// `true`. Con selección o varios cursores no hace nada (→ se inserta `\t`).
    pub fn try_expand_snippet(&mut self) -> bool {
        if self.cursors.len() != 1 {
            return false;
        }
        let primary = self.primary();
        if primary.has_selection() {
            return false;
        }
        let caret = primary.caret;
        let line_start = self.buffer.line_to_char(self.buffer.char_to_line(caret));
        let before = self.buffer.text_range(line_start..caret);
        let word_chars = before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .count();
        if word_chars == 0 {
            return false;
        }
        let skip = before.chars().count() - word_chars;
        let trigger = &before[before.char_indices().nth(skip).map_or(0, |(i, _)| i)..];
        let Some(body) = snippets::body_for(&self.lang, trigger) else {
            return false;
        };
        let word_start = caret - word_chars;
        let (text, caret_off) = snippets::render(body);
        self.with_transaction(|editor| {
            editor.buffer.delete(word_start..caret);
            editor.buffer.insert(word_start, &text);
            let pos = word_start + caret_off;
            editor.cursors[0].caret = pos;
            editor.cursors[0].anchor = pos;
        });
        self.reset_caret_blink();
        true
    }

    /// Líneas 0-based abarcadas por cualquier cursor (selección incluida),
    /// ordenadas y sin duplicados.
    fn comment_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        for cursor in &self.cursors {
            let range = cursor.range();
            let first = self.buffer.char_to_line(range.start);
            let last = if range.is_empty() {
                first
            } else {
                self.buffer.char_to_line(range.end.saturating_sub(1))
            };
            lines.extend(first..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    pub fn undo(&mut self) -> bool {
        if let Some(pairs) = self.history.undo(&mut self.buffer) {
            self.restore_cursors(pairs);
            self.scroll_into_view();
            self.reset_caret_blink();
            return true;
        }
        false
    }

    pub fn redo(&mut self) -> bool {
        if let Some(pairs) = self.history.redo(&mut self.buffer) {
            self.restore_cursors(pairs);
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
            // Shift+click: extiende la selección del cursor primario.
            (1, true) => {
                if let Some(primary) = self.cursors.first_mut() {
                    primary.caret = pos;
                }
            }
            (2, _) => {
                let range = movement::word_range(&self.buffer, pos);
                self.cursors = vec![Cursor {
                    caret: range.end,
                    anchor: range.start,
                }];
            }
            (3.., _) => {
                let line = self.buffer.char_to_line(pos);
                let range = movement::line_range(&self.buffer, line);
                self.cursors = vec![Cursor {
                    caret: range.end,
                    anchor: range.start,
                }];
            }
            _ => {
                self.cursors = vec![Cursor::new(pos)];
            }
        }
        self.sort_and_merge();
        self.scroll_into_view();
    }

    /// Alt+click: añade un cursor en la posición (sin tocar los demás).
    pub fn add_cursor_at(&mut self, x: f32, y: f32, advance: f32) {
        self.reset_caret_blink();
        if advance <= 0.0 {
            return;
        }
        let pos = self.caret_at(x, y, advance);
        self.cursors.push(Cursor::new(pos));
        self.sort_and_merge();
        self.scroll_into_view();
    }

    /// Ctrl+Alt+↑/↓: añade cursores arriba/abajo conservando la columna visual.
    /// Los duplicados se fusionan solos en [`Editor::sort_and_merge`].
    pub fn add_cursor_vertical(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let mut additions = Vec::with_capacity(self.cursors.len());
        let tab_width = self.tab_width;
        for cursor in &self.cursors {
            let line = self.buffer.char_to_line(cursor.caret);
            let target =
                (line as isize + delta).clamp(0, self.buffer.line_count() as isize - 1) as usize;
            let line_start = self.buffer.line_to_char(line);
            let text = self.buffer.line_text(line);
            let col = cursor.caret.saturating_sub(line_start);
            let visual = movement::visual_column(&text, col, tab_width);
            let target_text = self.buffer.line_text(target);
            let target_col = movement::char_col_at_visual(&target_text, visual, tab_width);
            additions.push(Cursor::new(self.buffer.line_to_char(target) + target_col));
        }
        self.cursors.extend(additions);
        self.sort_and_merge();
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Ctrl+D: sin selección, selecciona la palabra; con selección, añade la
    /// siguiente ocurrencia (dando la vuelta al final del buffer y saltando
    /// las que ya están seleccionadas).
    pub fn select_next_occurrence(&mut self) {
        self.sort_and_merge();
        if !self.cursors.iter().any(Cursor::has_selection) {
            let range = movement::word_range(&self.buffer, self.primary().caret);
            if range.is_empty() {
                return;
            }
            self.cursors = vec![Cursor {
                caret: range.end,
                anchor: range.start,
            }];
            self.scroll_into_view();
            return;
        }

        let needle = self
            .cursors
            .iter()
            .find(|cursor| cursor.has_selection())
            .map(|cursor| self.buffer.text_range(cursor.range()))
            .unwrap_or_default();
        if needle.is_empty() {
            return;
        }
        let needle_len = needle.chars().count();
        let mut search_from = self
            .cursors
            .iter()
            .map(|cursor| cursor.range().end)
            .max()
            .unwrap_or(0);

        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 10_000 {
                return;
            }
            let Some(pos) = self.find_wrapping(&needle, search_from) else {
                return;
            };
            let end = pos + needle_len;
            let covered = self.cursors.iter().any(|cursor| {
                let range = cursor.range();
                pos < range.end && range.start < end
            });
            if covered {
                search_from = pos + 1;
                continue;
            }
            self.cursors.push(Cursor {
                caret: end,
                anchor: pos,
            });
            self.sort_and_merge();
            self.scroll_into_view();
            return;
        }
    }

    /// Escape: deja un solo cursor (el primario) sin selección.
    pub fn collapse_to_primary(&mut self) {
        let caret = self.primary().caret;
        self.cursors = vec![Cursor::new(caret)];
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Extiende la selección primaria hasta el cursor (arrastre con el botón izquierdo).
    pub fn select_to(&mut self, x: f32, y: f32, advance: f32) {
        if advance <= 0.0 {
            return;
        }
        let pos = self.caret_at(x, y, advance);
        if let Some(primary) = self.cursors.first_mut() {
            primary.caret = pos;
        }
        self.sort_and_merge();
        self.scroll_into_view();
    }

    /// Rueda: `delta_y` en px lógicos positivos hacia arriba.
    pub fn on_wheel(&mut self, delta_y: f32) {
        self.scroll_y -= delta_y;
        self.clamp_scroll();
    }

    //── Buscar y reemplazar (Ctrl+F / Ctrl+H) ──────────────────────────────

    /// Texto de la selección primaria si cabe en una sola línea: la consulta
    /// inicial al abrir la barra con selección (lo usa `run_command`).
    pub fn single_line_selection(&self) -> Option<String> {
        let cursor = self.cursors.first()?;
        if !cursor.has_selection() {
            return None;
        }
        let text = self.buffer.text_range(cursor.range());
        (!text.contains('\n')).then_some(text)
    }

    /// Fija la consulta y recalcula las coincidencias (smart-case: escribe una
    /// mayúscula para que distinga mayúsculas; sin ella pliega el ASCII).
    /// La actual es la primera coincidencia en o después del caret, con vuelta.
    pub fn find_set_query(&mut self, query: &str) {
        self.find_query = query.to_owned();
        if query.is_empty() {
            self.find.clear();
            self.find_current = 0;
            self.find_revision = self.buffer.revision();
            return;
        }
        let from = self.primary().caret;
        self.recompute_find(from);
        self.reveal_current_match();
    }

    /// Resincroniza `find` si el buffer cambió desde su cálculo (undo/redo,
    /// edición…). Lo llaman build_scene y la navegación, así que el estado
    /// nunca queda viejo más de un frame.
    fn find_sync(&mut self) {
        if self.find_revision == self.buffer.revision() {
            return;
        }
        if self.find_query.is_empty() {
            self.find.clear();
            self.find_current = 0;
        } else {
            let from = self
                .find
                .get(self.find_current)
                .map_or_else(|| self.primary().caret, |range| range.start);
            self.recompute_find(from);
        }
        self.find_revision = self.buffer.revision();
    }

    /// Recalcula `find` y deja como actual la primera coincidencia en o
    /// después de `from` (si no hay, la primera).
    fn recompute_find(&mut self, from: usize) {
        self.find = find_in_buffer(&self.buffer, &self.find_query);
        self.find_current = self
            .find
            .iter()
            .position(|range| range.start >= from)
            .unwrap_or(0);
        self.find_revision = self.buffer.revision();
    }

    /// Siguiente (o anterior con `backwards`) coincidencia, con vuelta al
    /// otro extremo; la saca a la vista.
    pub fn find_next(&mut self, backwards: bool) {
        self.find_sync();
        if self.find.is_empty() {
            return;
        }
        let last = self.find.len() - 1;
        self.find_current = self.find_current.min(last);
        self.find_current = if backwards {
            self.find_current.checked_sub(1).unwrap_or(last)
        } else {
            (self.find_current + 1) % (last + 1)
        };
        self.reveal_current_match();
    }

    /// Cierra la búsqueda; la coincidencia actual queda como selección.
    pub fn find_close(&mut self) {
        if let Some(range) = self.find.get(self.find_current).cloned() {
            self.cursors = vec![Cursor {
                caret: range.end,
                anchor: range.start,
            }];
            self.scroll_into_view();
        }
        self.find.clear();
        self.find_current = 0;
        self.find_query.clear();
        self.find_revision = self.buffer.revision();
        self.reset_caret_blink();
    }

    /// Reemplaza la coincidencia actual en su transacción de undo y avanza a
    /// la siguiente a partir del texto nuevo (con vuelta). `false` si no hay.
    pub fn find_replace_current(&mut self, replacement: &str) -> bool {
        self.find_sync();
        let Some(range) = self.find.get(self.find_current).cloned() else {
            return false;
        };
        let start = range.start;
        self.with_transaction(|editor| editor.replace_match(&range, replacement));
        self.recompute_find(start + replacement.chars().count());
        self.reveal_current_match();
        true
    }

    /// Reemplaza TODAS las coincidencias de derecha a izquierda (los offsets
    /// altos no afectan a los bajos) en UNA transacción de undo.
    /// Devuelve cuántas había.
    pub fn find_replace_all(&mut self, replacement: &str) -> usize {
        self.find_sync();
        if self.find.is_empty() {
            return 0;
        }
        let matches = self.find.clone();
        let count = matches.len();
        self.with_transaction(|editor| {
            for range in matches.iter().rev() {
                editor.replace_match(range, replacement);
            }
        });
        self.recompute_find(0);
        self.reset_caret_blink();
        count
    }

    /// (coincidencia actual 1-based, total) para el contador de la barra.
    pub fn find_stats(&self) -> (usize, usize) {
        let total = self.find.len();
        let current = if total == 0 { 0 } else { self.find_current + 1 };
        (current, total)
    }

    /// Borra `range` e inserta `replacement` en su sitio, desplazando todos
    /// los cursores (lo que quedaba dentro del rango colapsa al principio).
    fn replace_match(&mut self, range: &Range<usize>, replacement: &str) {
        let (start, end) = (range.start, range.end);
        let inserted = replacement.chars().count();
        self.buffer.delete(start..end);
        self.buffer.insert(start, replacement);
        for cursor in &mut self.cursors {
            cursor.caret = shift_after(cursor.caret, start, end, inserted);
            cursor.anchor = shift_after(cursor.anchor, start, end, inserted);
        }
    }

    /// Sitúa la coincidencia actual a la vista y reinicia el parpadeo.
    fn reveal_current_match(&mut self) {
        if let Some(start) = self.find.get(self.find_current).map(|range| range.start) {
            self.scroll_position_into_view(start);
        }
        self.reset_caret_blink();
    }

    /// Escena de este frame: selecciones + gutter + líneas visibles + carets.
    /// Resalta lo que haga falta (cache) hasta la última línea visible.
    /// `viewport` es el área del EDITOR (la ventana menos la barra de pestañas).
    pub fn build_scene(&mut self, viewport: (f32, f32), advance_ratio: f32) -> Scene {
        let bg = self.theme.editor_bg;
        let mut scene = Scene::new(srgb(bg.0, bg.1, bg.2));
        let advance = FONT_SIZE * advance_ratio;
        let line_count = self.buffer.line_count();
        let inset = self.top_inset;
        let xin = self.left_inset;
        let cx = xin + self.code_offset();
        let tab_width = self.tab_width;

        let visible_first = (((self.scroll_y - inset - PADDING_TOP) / LINE_HEIGHT)
            .floor()
            .max(0.0) as usize)
            .min(line_count - 1);
        let visible_last = (((self.scroll_y + viewport.1 - inset - PADDING_TOP) / LINE_HEIGHT)
            .ceil() as usize)
            .min(line_count - 1);

        self.syntax.set_extension(&self.lang);
        self.syntax.prepare(&mut self.buffer, visible_last);
        // La búsqueda se resincroniza si el buffer cambió (undo, edición…).
        self.find_sync();
        let ctx = RangeCtx {
            code_x: cx,
            top_inset: inset,
            visible_first,
            visible_last,
            scroll_y: self.scroll_y,
            advance,
            tab_width,
        };
        for cursor in &self.cursors {
            let range = cursor.range();
            push_range_quad(&mut scene, &range, &self.buffer, ctx, self.theme.selection);
        }
        // Coincidencias de la búsqueda (Ctrl+F): apagadas y, en su turno,
        // la actual por encima (no se solapan: rangos disjuntos).
        for (index, range) in self.find.iter().enumerate() {
            let color = if index == self.find_current {
                self.theme.current_match
            } else {
                self.theme.match_bg
            };
            push_range_quad(&mut scene, range, &self.buffer, ctx, color);
        }

        let caret_line = self.buffer.char_to_line(self.primary().caret);
        for line in visible_first..=visible_last {
            let y = inset + PADDING_TOP + line as f32 * LINE_HEIGHT - self.scroll_y;
            if self.show_line_numbers {
                let color = if line == caret_line {
                    self.theme.gutter_current
                } else {
                    self.theme.gutter_fg
                };
                let number = format!("{}", line + 1);
                let number_x = xin + GUTTER_RIGHT_X - number.len() as f32 * advance;
                scene
                    .texts
                    .push(TextSpan::new(number, number_x, y, FONT_SIZE, color));
            }

            let text = self.buffer.line_text(line);
            if text.is_empty() {
                continue;
            }
            let tokens = self.syntax.tokens(line);
            if tokens.is_empty() {
                scene
                    .texts
                    .push(TextSpan::new(text, cx, y, FONT_SIZE, self.theme.fg));
                continue;
            }
            // Columnas de TODOS los tokens en un solo recorrido de la línea
            // (antes: visual_column por token = O(tokens × columna) por frame).
            let starts: Vec<usize> = tokens.iter().map(|token| token.start).collect();
            let columns = movement::visual_columns_at(&text, &starts, tab_width);
            for (token, &cells) in tokens.iter().zip(&columns) {
                let piece = text
                    .get(token.start..token.end.min(text.len()))
                    .unwrap_or("");
                if piece.is_empty() || piece.chars().all(|c| c == '\n') {
                    continue;
                }
                scene.texts.push(TextSpan::new(
                    piece,
                    cx + cells as f32 * advance,
                    y,
                    FONT_SIZE,
                    token.color,
                ));
            }
        }

        if self.focused && self.caret_on {
            for cursor in &self.cursors {
                let line = self.buffer.char_to_line(cursor.caret);
                if line < visible_first || line > visible_last {
                    continue;
                }
                let y = inset + PADDING_TOP + line as f32 * LINE_HEIGHT - self.scroll_y;
                let col = cursor.caret - self.buffer.line_to_char(line);
                let cells = movement::visual_column(&self.buffer.line_text(line), col, tab_width);
                scene.overlays.push(Quad::new(
                    cx + cells as f32 * advance,
                    y,
                    CARET_WIDTH,
                    LINE_HEIGHT - 2.0,
                    self.theme.caret,
                ));
            }
        }
        scene
    }

    /// Cursor primario (izquierdo); defensa ante una lista vacía.
    fn primary(&self) -> Cursor {
        self.cursors.first().copied().unwrap_or(Cursor::new(0))
    }

    fn cursor_pairs(&self) -> Vec<(usize, usize)> {
        self.cursors.iter().map(|c| (c.caret, c.anchor)).collect()
    }

    /// Ordena los cursores y fusiona los que se solapan o tocan (la fusión
    /// resulta siempre en una selección hacia delante que cubre la unión).
    /// Mantiene la invariante ≥ 1 cursor y rangos disjuntos en orden creciente.
    fn sort_and_merge(&mut self) {
        self.cursors
            .sort_by_key(|cursor| (cursor.range().start, cursor.range().end));
        let mut merged: Vec<Cursor> = Vec::with_capacity(self.cursors.len());
        for cursor in self.cursors.drain(..) {
            let overlaps = merged
                .last()
                .is_some_and(|prev| cursor.range().start <= prev.range().end);
            if overlaps {
                let prev = merged.last_mut().expect("overlaps implica previo");
                let start = prev.range().start.min(cursor.range().start);
                let end = prev.range().end.max(cursor.range().end);
                prev.caret = end;
                prev.anchor = start;
            } else {
                merged.push(cursor);
            }
        }
        if merged.is_empty() {
            merged.push(Cursor::new(0));
        }
        self.cursors = merged;
    }

    /// Ejecuta `body` (ediciones al buffer + cursores) y lo registra como UNA
    /// transacción de undo con los carets antes/después.
    fn with_transaction(&mut self, body: impl FnOnce(&mut Self)) {
        self.sort_and_merge();
        let before = self.cursor_pairs();
        self.buffer.begin();
        body(self);
        self.sort_and_merge();
        let edits = self.buffer.take_edits();
        let after = self.cursor_pairs();
        self.history.commit(Transaction::new(edits, before, after));
        self.scroll_into_view();
    }

    fn restore_cursors(&mut self, pairs: Vec<(usize, usize)>) {
        let len = self.buffer.len_chars();
        self.cursors = pairs
            .into_iter()
            .map(|(caret, anchor)| Cursor {
                caret: caret.min(len),
                anchor: anchor.min(len),
            })
            .collect();
        self.sort_and_merge();
    }

    /// Siguiente ocurrencia de `needle` a partir de `from` (con vuelta al
    /// inicio). Las agujas multilínea buscan en el texto completo.
    fn find_wrapping(&self, needle: &str, from: usize) -> Option<usize> {
        let len = self.buffer.len_chars();
        if needle.is_empty() || len == 0 {
            return None;
        }
        let from = from.min(len);

        if needle.contains('\n') {
            let hay = self.buffer.text_range(0..len);
            let start = movement::byte_at_char(&hay, from);
            let found = hay[start..]
                .find(needle)
                .map(|rel| start + rel)
                .or_else(|| hay[..start].find(needle));
            return found.map(|byte| hay[..byte].chars().count());
        }

        // Búsqueda por líneas: de `from` al final y, si no, del inicio a `from`.
        let line_count = self.buffer.line_count();
        let start_line = self.buffer.char_to_line(from);
        let start_col = from - self.buffer.line_to_char(start_line);
        for line in start_line..line_count {
            let text = self.buffer.line_text(line);
            let from_col = if line == start_line { start_col } else { 0 };
            if let Some(col) = find_in_text(&text, from_col, needle) {
                return Some(self.buffer.line_to_char(line) + col);
            }
        }
        for line in 0..=start_line {
            let text = self.buffer.line_text(line);
            let found =
                find_in_text(&text, 0, needle).filter(|&col| line < start_line || col < start_col);
            if let Some(col) = found {
                return Some(self.buffer.line_to_char(line) + col);
            }
        }
        None
    }

    /// Movimiento de todos los cursores con una función de destino.
    fn move_cursors(&mut self, target: impl Fn(&Cursor, &Buffer) -> usize, shift: bool) {
        for cursor in &mut self.cursors {
            let next = target(cursor, &self.buffer);
            cursor.caret = next;
            if !shift {
                cursor.anchor = next;
            }
        }
        self.sort_and_merge();
        self.scroll_into_view();
    }

    /// Coordenadas lógicas → offset de char (columna visual, media celda).
    fn caret_at(&self, x: f32, y: f32, advance: f32) -> usize {
        let visual = ((x - self.left_inset - self.code_offset()) / advance)
            .floor()
            .max(0.0) as usize;
        let line_f = ((y - self.top_inset - PADDING_TOP + self.scroll_y) / LINE_HEIGHT).floor();
        let line = (line_f.max(0.0) as usize).min(self.buffer.line_count() - 1);
        let text = self.buffer.line_text(line);
        let char_col = movement::char_col_at_visual(&text, visual, self.tab_width);
        self.buffer.line_to_char(line) + char_col
    }

    fn page_lines(&self) -> isize {
        ((self.viewport.1 / LINE_HEIGHT) as isize).max(1)
    }

    fn scroll_into_view(&mut self) {
        let caret = self.primary().caret;
        self.scroll_position_into_view(caret);
    }

    /// Garantiza que la posición `pos` (un offset de char) quede a la vista.
    fn scroll_position_into_view(&mut self, pos: usize) {
        let line = self.buffer.char_to_line(pos);
        let top = self.top_inset + PADDING_TOP + line as f32 * LINE_HEIGHT;
        let bottom = top + LINE_HEIGHT;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + self.viewport.1 {
            self.scroll_y = bottom - self.viewport.1;
        }
        self.clamp_scroll();
    }

    /// ¿El caret primario está a la vista? Si no lo está, el parpadeo puede
    /// saltarse el repintar (el estado de blink se conserva).
    pub fn caret_in_view(&self) -> bool {
        let line = self.buffer.char_to_line(self.primary().caret);
        let top = self.top_inset + PADDING_TOP + line as f32 * LINE_HEIGHT;
        let bottom = top + LINE_HEIGHT;
        bottom > self.scroll_y && top < self.scroll_y + self.viewport.1
    }

    fn clamp_scroll(&mut self) {
        let content_h = self.top_inset
            + PADDING_TOP
            + self.buffer.line_count() as f32 * LINE_HEIGHT
            + PADDING_TOP;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - self.viewport.1).max(0.0));
    }
}

/// Intersección de las líneas `first..=last` con el rango visible
/// `visible_first..=visible_last`. Si no se solapan, el rango devuelto queda
/// invertido (empieza tras su fin) y por tanto vacío al iterarlo.
fn visible_line_range(
    first: usize,
    last: usize,
    visible_first: usize,
    visible_last: usize,
) -> std::ops::RangeInclusive<usize> {
    first.max(visible_first)..=last.min(visible_last)
}

/// Contexto de dibujo de un rango (parámetros fijos del frame) para no
/// pasar ocho argumentos a [`push_range_quad`]. `code_x` ya incluye el
/// inset izquierdo y el gutter (visible u oculto).
#[derive(Clone, Copy)]
struct RangeCtx {
    code_x: f32,
    top_inset: f32,
    visible_first: usize,
    visible_last: usize,
    scroll_y: f32,
    advance: f32,
    tab_width: usize,
}

/// Quad de selección/coincidencia para `range`, recortado a las líneas
/// visibles: una sola implementación para selecciones y para la búsqueda.
fn push_range_quad(
    scene: &mut Scene,
    range: &Range<usize>,
    buffer: &Buffer,
    ctx: RangeCtx,
    color: (u8, u8, u8, u8),
) {
    if range.is_empty() {
        return;
    }
    let RangeCtx {
        code_x,
        top_inset: inset,
        visible_first,
        visible_last,
        scroll_y,
        advance,
        tab_width,
    } = ctx;
    let first = buffer.char_to_line(range.start);
    let last = buffer.char_to_line(range.end.saturating_sub(1));
    // Solo la intersección con lo visible: O(viewport), no O(rango).
    for line in visible_line_range(first, last, visible_first, visible_last) {
        let y = inset + PADDING_TOP + line as f32 * LINE_HEIGHT - scroll_y;
        let text = buffer.line_text(line);
        let line_start = buffer.line_to_char(line);

        let start_cells = if line == first {
            let col = range.start - line_start;
            movement::visual_column(&text, col, tab_width)
        } else {
            0
        };
        let end_cells = if line == last {
            let col = range.end - line_start;
            // Seleccionar el \n añade una celda más (como Sublime).
            if col > text.chars().count() {
                movement::visual_column(&text, text.chars().count(), tab_width) + 1
            } else {
                movement::visual_column(&text, col, tab_width)
            }
        } else {
            movement::visual_column(&text, text.chars().count(), tab_width)
        };

        let x = code_x + start_cells as f32 * advance;
        let w = (end_cells.saturating_sub(start_cells)) as f32 * advance;
        if w > 0.0 {
            scene.quads.push(Quad::new(x, y, w, LINE_HEIGHT, color));
        }
    }
}

/// Todas las posiciones (chars) de `query` en el buffer, en orden.
///
/// smart-case: si `query` trae mayúsculas, distingue mayúsculas; si no,
/// pliega SOLO el ASCII de ambos lados (los offsets byte no cambian, así que
/// los rangos siguen siendo válidos; lo no-ASCII no se case-foldea, decisión
/// documentada). Las agujas multilínea buscan en el texto entero.
fn find_in_buffer(buffer: &Buffer, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let hay = buffer.text_range(0..buffer.len_chars());
    let (hay_cmp, needle): (Cow<str>, Cow<str>) = if query.chars().any(char::is_uppercase) {
        (Cow::Borrowed(hay.as_str()), Cow::Borrowed(query))
    } else {
        (
            Cow::Owned(hay.to_ascii_lowercase()),
            Cow::Owned(query.to_ascii_lowercase()),
        )
    };
    let needle_chars = needle.chars().count();
    let mut found = Vec::new();
    let mut prev_byte = 0;
    let mut prev_chars = 0;
    // Conversión byte→char incremental: O(n) por consulta, no O(n) por match.
    for (byte, _) in hay_cmp.match_indices(needle.as_ref()) {
        let chars = prev_chars + hay_cmp[prev_byte..byte].chars().count();
        found.push(chars..chars + needle_chars);
        prev_byte = byte;
        prev_chars = chars;
    }
    found
}

/// Mapeo de una posición tras borrar `start..end` e insertar `inserted` chars
/// en `start`: lo que quedaba dentro del rango borrado colapsa al inicio.
fn shift_after(pos: usize, start: usize, end: usize, inserted: usize) -> usize {
    if pos < start {
        pos
    } else if pos < end {
        start + inserted
    } else {
        pos - (end - start) + inserted
    }
}

/// Prefijo de comentario de línea según la extensión del archivo (default `//`).
fn comment_prefix(lang: &str) -> &'static str {
    match lang {
        "py" | "sh" | "bash" | "rb" | "yaml" | "yml" | "toml" | "pl" | "ps1" | "r" | "mk" => "#",
        "sql" | "lua" | "hs" | "elm" => "--",
        "html" | "xml" | "svg" => "<!--",
        _ => "//",
    }
}

/// Texto de una línea sin su final (`\n` o `\r\n`).
fn trim_line_ending(text: &str) -> &str {
    text.strip_suffix('\n')
        .map(|t| t.strip_suffix('\r').unwrap_or(t))
        .unwrap_or(text)
}

/// Primera ocurrencia de `needle` en `text` a partir del char `from_col`
/// (la aguja debe caber entera en `text`); devuelve su posición en chars.
fn find_in_text(text: &str, from_col: usize, needle: &str) -> Option<usize> {
    let start = movement::byte_at_char(text, from_col);
    let rel = text.get(start..)?.find(needle)?;
    Some(text[..start + rel].chars().count())
}

/// Archivos pasados por CLI (uno por pestaña); vacío → un editor en blanco.
pub fn cli_paths() -> Vec<PathBuf> {
    std::env::args_os().skip(1).map(PathBuf::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(editor: &Editor) -> String {
        editor.buffer.text_range(0..editor.buffer.len_chars())
    }

    #[test]
    fn typing_joins_into_one_undo_step() {
        let mut editor = Editor::default();
        for ch in "hola".chars() {
            editor.insert_text(&ch.to_string());
        }
        assert_eq!(text_of(&editor), "hola");

        assert!(editor.undo());
        assert_eq!(text_of(&editor), "");
        assert!(editor.redo());
        assert_eq!(text_of(&editor), "hola");
        assert_eq!(editor.cursors[0].caret, 4);
    }

    #[test]
    fn replace_selection_is_one_atomic_undo() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        editor.cursors = vec![Cursor {
            caret: 0,
            anchor: 4,
        }]; // selecciona "hola"
        editor.insert_text("x");
        assert_eq!(text_of(&editor), "x mundo");

        assert!(editor.undo());
        assert_eq!(text_of(&editor), "hola mundo");
        assert_eq!(
            (editor.cursors[0].caret, editor.cursors[0].anchor),
            (0, 4),
            "restaura carets"
        );
    }

    #[test]
    fn backspace_after_undo_redo_run() {
        let mut editor = Editor::default();
        editor.insert_text("abc");
        editor.backspace();
        editor.backspace();
        assert_eq!(text_of(&editor), "a");
        assert!(editor.undo(), "los backspaces se deshacen juntos");
        assert_eq!(text_of(&editor), "abc");
    }

    #[test]
    fn copy_cut_and_paste_roundtrip() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        editor.cursors = vec![Cursor {
            caret: 0,
            anchor: 4,
        }];

        assert_eq!(editor.copy().as_deref(), Some("hola"));
        let cut = editor.cut().expect("cut debe devolver el texto");
        assert_eq!(cut, "hola");
        assert_eq!(text_of(&editor), " mundo");

        editor.cursors = vec![Cursor::new(editor.buffer.len_chars())];
        editor.insert_text(&cut);
        assert_eq!(text_of(&editor), " mundohola");
        assert!(editor.undo(), "el pegado es un solo undo");
        assert_eq!(text_of(&editor), " mundo");
    }

    #[test]
    fn double_and_triple_click_select_word_and_line() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        let advance = 10.0;

        // Doble click sobre "hola" (x=0) → palabra.
        editor.on_click(CODE_X, 0.0, advance, 2, false);
        assert_eq!(editor.primary().range(), 0..4);

        // Triple click → línea completa (con \n si existe; aquí no lo hay).
        editor.on_click(CODE_X, 0.0, advance, 3, false);
        assert_eq!(editor.primary().range(), 0..10);

        // Con fin de línea, la selección incluye el \n.
        let mut editor = Editor::default();
        editor.insert_text("hola\nmundo");
        editor.on_click(CODE_X, 0.0, advance, 3, false);
        assert_eq!(editor.primary().range(), 0..5);
    }

    #[test]
    fn shift_arrow_extends_and_plain_arrow_collapses() {
        let mut editor = Editor::default();
        editor.insert_text("abc");
        editor.cursors = vec![Cursor::new(0)];

        let shift = ModifiersState::SHIFT;
        let right = Key::Named(NamedKey::ArrowRight);
        editor.handle_key(&right, shift);
        assert_eq!(editor.primary().range(), 0..1);
        editor.handle_key(&right, shift);
        assert_eq!(editor.primary().range(), 0..2);

        // Sin Shift, colapsa al borde izquierdo.
        editor.handle_key(&Key::Named(NamedKey::ArrowLeft), ModifiersState::empty());
        assert!(!editor.cursors.iter().any(Cursor::has_selection));
    }

    #[test]
    fn syntax_colors_split_code_lines_into_tokens() {
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
        editor.insert_text("fn main() {}");

        let scene = editor.build_scene((1200.0, 800.0), 0.6);
        let code: Vec<_> = scene.texts.iter().filter(|span| span.x >= CODE_X).collect();
        assert!(
            code.len() >= 2,
            "la línea Rust parte en tokens: {}",
            code.len()
        );
        let distinct: std::collections::HashSet<_> = code.iter().map(|span| span.color).collect();
        assert!(
            distinct.len() >= 2,
            "los tokens deben llevar colores distintos: {distinct:?}"
        );
    }

    #[test]
    fn multi_cursor_edits_apply_everywhere() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        // Dos cursores: "hola" (0..4) y "mundo" (5..10).
        editor.cursors = vec![
            Cursor {
                caret: 4,
                anchor: 0,
            },
            Cursor {
                caret: 10,
                anchor: 5,
            },
        ];

        editor.insert_text("X");
        assert_eq!(text_of(&editor), "X X", "escribe en los dos sitios");
        assert_eq!(editor.cursors.len(), 2, "un cursor por selección");
        assert_eq!(
            editor.cursors.iter().map(|c| c.caret).collect::<Vec<_>>(),
            vec![1, 3]
        );

        assert!(editor.undo(), "una sola transacción para ambos edits");
        assert_eq!(text_of(&editor), "hola mundo");
        assert_eq!(
            editor
                .cursors
                .iter()
                .map(|c| (c.caret, c.anchor))
                .collect::<Vec<_>>(),
            vec![(4, 0), (10, 5)],
            "undo restaura TODOS los carets"
        );
        assert!(editor.redo());
        assert_eq!(text_of(&editor), "X X");
    }

    #[test]
    fn ctrl_d_selects_word_then_next_occurrence() {
        let mut editor = Editor::default();
        editor.insert_text("foo bar foo");
        editor.cursors = vec![Cursor::new(0)];

        editor.select_next_occurrence();
        assert_eq!(
            editor.primary().range(),
            0..3,
            "primero selecciona la palabra"
        );

        editor.select_next_occurrence();
        assert_eq!(editor.cursors.len(), 2);
        let ranges: Vec<_> = editor.cursors.iter().map(Cursor::range).collect();
        assert_eq!(ranges, vec![0..3, 8..11], "añade la siguiente ocurrencia");

        editor.select_next_occurrence();
        assert_eq!(
            editor.cursors.len(),
            2,
            "las dos ya están cubiertas: no añade más"
        );
    }

    #[test]
    fn add_cursor_vertical_dedups_repeated_lines() {
        let mut editor = Editor::default();
        editor.insert_text("ab\ncd\nef");
        editor.cursors = vec![Cursor::new(4)]; // línea 1, columna 1

        editor.add_cursor_vertical(-1);
        assert_eq!(editor.cursors.len(), 2);
        assert_eq!(
            editor.cursors.iter().map(|c| c.caret).collect::<Vec<_>>(),
            vec![1, 4],
            "conserva la columna visual"
        );

        editor.add_cursor_vertical(-1);
        assert_eq!(
            editor.cursors.len(),
            2,
            "el cursor duplicado de la línea 0 se fusiona"
        );
    }

    #[test]
    fn delete_current_line_drops_line_and_undoes() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres");
        editor.cursors = vec![Cursor::new(5)]; // sobre "dos"

        editor.delete_current_line();
        assert_eq!(text_of(&editor), "uno\ntres");
        assert_eq!(editor.cursors[0].caret, 4, "cae al inicio de 'tres'");

        assert!(editor.undo());
        assert_eq!(text_of(&editor), "uno\ndos\ntres");
        assert_eq!(editor.cursors[0].caret, 5, "restaura el caret");
    }

    #[test]
    fn delete_current_line_multi_cursor_dedups_shared_line() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres");
        editor.cursors = vec![Cursor::new(5), Cursor::new(6)]; // ambos en "dos"

        editor.delete_current_line();
        assert_eq!(
            text_of(&editor),
            "uno\ntres",
            "la línea solo se borra una vez"
        );
        assert_eq!(editor.cursors.len(), 1);
        assert_eq!(editor.cursors[0].caret, 4);
    }

    #[test]
    fn delete_current_line_last_line_keeps_newline_before() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos");
        editor.cursors = vec![Cursor::new(5)]; // sobre "dos", la última línea

        editor.delete_current_line();
        assert_eq!(text_of(&editor), "uno\n");
        assert_eq!(editor.cursors[0].caret, 4);
    }

    #[test]
    fn delete_current_line_on_empty_last_line_removes_newline() {
        let mut editor = Editor::default();
        editor.insert_text("a\n");
        editor.cursors = vec![Cursor::new(2)]; // línea 1, vacía

        editor.delete_current_line();
        assert_eq!(text_of(&editor), "a");
        assert_eq!(editor.cursors[0].caret, 1);
    }

    #[test]
    fn duplicate_line_inserts_copy_keeps_cursor_and_undoes() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos");
        editor.cursors = vec![Cursor::new(1)]; // en "uno"

        editor.duplicate_current_line();
        assert_eq!(text_of(&editor), "uno\nuno\ndos");
        assert_eq!(editor.cursors[0].caret, 1, "el caret queda en la original");

        assert!(editor.undo());
        assert_eq!(text_of(&editor), "uno\ndos");
        assert_eq!(editor.cursors[0].caret, 1);
    }

    #[test]
    fn duplicate_last_line_appends_copy() {
        let mut editor = Editor::default();
        editor.insert_text("a\nbb");
        editor.cursors = vec![Cursor::new(3)]; // en "bb", última línea

        editor.duplicate_current_line();
        assert_eq!(text_of(&editor), "a\nbb\nbb");
        assert_eq!(editor.cursors[0].caret, 3, "caret en la original");
    }

    #[test]
    fn altgr_writes_symbols_but_ctrl_is_shortcut() {
        let mut editor = Editor::default();

        // AltGr (Ctrl+Alt) en Windows escribe símbolos: no se traga.
        let ctrl_alt = ModifiersState::CONTROL | ModifiersState::ALT;
        assert!(editor.handle_key(&Key::Character("@".into()), ctrl_alt));
        assert_eq!(text_of(&editor), "@");

        // Ctrl solo: el shell de la app maneja el atajo (aquí se ignora).
        assert!(!editor.handle_key(&Key::Character("s".into()), ModifiersState::CONTROL));
        assert_eq!(text_of(&editor), "@");
    }

    #[test]
    fn visible_line_range_intersects_or_goes_empty() {
        assert_eq!(
            visible_line_range(0, 10, 5, 15).collect::<Vec<_>>(),
            vec![5, 6, 7, 8, 9, 10],
            "solape por la izquierda"
        );
        assert_eq!(
            visible_line_range(12, 20, 5, 15).collect::<Vec<_>>(),
            vec![12, 13, 14, 15],
            "solape por la derecha"
        );
        assert!(
            visible_line_range(100, 200, 5, 15).next().is_none(),
            "entera por debajo del viewport"
        );
        assert!(
            visible_line_range(0, 3, 5, 15).next().is_none(),
            "entera por encima del viewport"
        );
        assert!(
            visible_line_range(30, 10, 5, 15).next().is_none(),
            "rango invertido sigue vacío"
        );
    }

    #[test]
    fn caret_in_view_tracks_scroll() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres");
        editor.set_viewport(400.0, 100.0); // ~5 líneas visibles
        editor.cursors = vec![Cursor::new(0)];
        assert!(editor.caret_in_view());

        editor.scroll_y = 10_000.0;
        assert!(!editor.caret_in_view(), "scroll lejos: no hay que repintar");
    }

    #[test]
    fn select_all_scene_renders_only_visible_selection() {
        let mut editor = Editor::default();
        // 100k líneas: con la intersección visible el bucle es O(viewport).
        editor.insert_text(&"pub fn f() {}\n".repeat(100_000));
        editor.set_viewport(600.0, 200.0);
        editor.select_all();
        editor.scroll_y = 0.0; // arriba del todo: filas 0..=10 exactas

        let scene = editor.build_scene((600.0, 200.0), 1.0);
        // PADDING_TOP=8 y LINE_HEIGHT=20 con viewport 200 → líneas 0..=10.
        assert_eq!(
            scene.quads.len(),
            11,
            "un quad de selección por línea visible, no por línea del archivo"
        );
    }

    #[test]
    fn find_set_query_smart_case_and_counts() {
        let mut editor = Editor::default();
        editor.insert_text("Foo bar foo FOO");
        editor.cursors = vec![Cursor::new(0)];

        editor.find_set_query("foo");
        assert_eq!(
            editor.find_stats(),
            (1, 3),
            "sin mayúsculas: case-insensitive"
        );

        editor.find_set_query("Foo");
        assert_eq!(
            editor.find_stats(),
            (1, 1),
            "con mayúscula: smart-case sensible"
        );
        editor.find_set_query("FOO");
        assert_eq!(editor.find_stats(), (1, 1));

        editor.find_set_query("");
        assert_eq!(
            editor.find_stats(),
            (0, 0),
            "consulta vacía sin coincidencias"
        );
    }

    #[test]
    fn find_next_wraps_and_shift_goes_backwards() {
        let mut editor = Editor::default();
        editor.insert_text("a a a");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("a");
        assert_eq!(editor.find_stats(), (1, 3));

        editor.find_next(false);
        assert_eq!(editor.find_stats(), (2, 3));
        editor.find_next(true);
        assert_eq!(editor.find_stats(), (1, 3));
        editor.find_next(true);
        assert_eq!(editor.find_stats(), (3, 3), "hacia atrás con vuelta");
        editor.find_next(false);
        assert_eq!(editor.find_stats(), (1, 3), "hacia delante con vuelta");
    }

    #[test]
    fn find_close_selects_current_match() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo hola");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("mundo");
        assert_eq!(editor.find_stats(), (1, 1));

        editor.find_close();
        assert_eq!(
            editor.primary().range(),
            5..10,
            "selecciona la coincidencia"
        );
        assert_eq!(editor.find_stats(), (0, 0), "la búsqueda queda cerrada");
    }

    #[test]
    fn find_replace_current_advances_and_undoes() {
        let mut editor = Editor::default();
        editor.insert_text("uno dos uno");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("uno");

        assert!(editor.find_replace_current("1"));
        assert_eq!(text_of(&editor), "1 dos uno");
        assert_eq!(
            editor.find_stats(),
            (1, 1),
            "queda el segundo y es el actual"
        );

        assert!(editor.find_replace_current("1"));
        assert_eq!(text_of(&editor), "1 dos 1");
        assert_eq!(editor.find_stats(), (0, 0), "sin coincidencias");
        assert!(!editor.find_replace_current("x"), "no hay qué reemplazar");

        assert!(editor.undo(), "cada reemplazo es su propia transacción");
        assert_eq!(text_of(&editor), "1 dos uno");
        assert!(editor.undo());
        assert_eq!(text_of(&editor), "uno dos uno");
    }

    #[test]
    fn find_replace_all_is_one_undo() {
        let mut editor = Editor::default();
        editor.insert_text("a-a-a");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("a");

        assert_eq!(editor.find_replace_all("X"), 3, "cuenta las tres");
        assert_eq!(text_of(&editor), "X-X-X");
        assert!(editor.undo(), "un solo undo para todo");
        assert_eq!(text_of(&editor), "a-a-a");
        assert_eq!(
            (editor.cursors[0].caret, editor.cursors[0].anchor),
            (0, 0),
            "restaura el caret"
        );
    }

    #[test]
    fn find_sync_after_undo_recomputes_matches() {
        let mut editor = Editor::default();
        editor.insert_text("aa");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("a");
        assert_eq!(editor.find_stats(), (1, 2));

        editor.find_replace_all("b");
        assert_eq!(text_of(&editor), "bb");
        assert_eq!(editor.find_stats(), (0, 0), "las coincidencias desaparecen");

        assert!(editor.undo());
        assert_eq!(text_of(&editor), "aa");
        editor.find_next(false); // la revisión nueva dispara find_sync
        assert_eq!(editor.find_stats(), (2, 2), "resincronizado tras undo");
    }

    #[test]
    fn find_multiline_and_single_line_selection() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ndos\nfin");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("dos\ndos");
        assert_eq!(
            editor.find_stats(),
            (1, 1),
            "la aguja multilínea busca en todo el texto"
        );

        // La selección multilínea no precarga la barra; la de una línea, sí.
        editor.cursors = vec![Cursor {
            caret: 7,
            anchor: 0,
        }];
        assert_eq!(editor.single_line_selection(), None);
        editor.cursors = vec![Cursor {
            caret: 3,
            anchor: 0,
        }];
        assert_eq!(editor.single_line_selection().as_deref(), Some("uno"));
        editor.cursors = vec![Cursor::new(0)];
        assert_eq!(editor.single_line_selection(), None, "sin selección");
    }

    #[test]
    fn match_quads_draw_above_selections() {
        let mut editor = Editor::default();
        editor.insert_text("hola hola");
        editor.cursors = vec![Cursor::new(0)];
        editor.find_set_query("hola");

        let scene = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(scene.quads.len(), 2, "una por coincidencia");
        assert_eq!(
            scene.quads[0].color,
            Theme::dark().current_match,
            "la actual la última…"
        );
        assert_eq!(
            scene.quads[1].color,
            Theme::dark().match_bg,
            "…y por encima de las demás"
        );

        // Con la palabra seleccionada (prefill de Ctrl+F) el quad actual
        // tapa al de selección: se dibuja después.
        editor.cursors = vec![Cursor {
            caret: 4,
            anchor: 0,
        }];
        let scene = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(scene.quads.len(), 3);
        assert_eq!(scene.quads[0].color, Theme::dark().selection);
        assert_eq!(scene.quads[1].color, Theme::dark().current_match);
        assert_eq!(scene.quads[2].color, Theme::dark().match_bg);
    }

    #[test]
    fn build_scene_follows_the_active_theme() {
        let mut editor = Editor::default();
        editor.insert_text("fn x() {}");
        editor.cursors = vec![Cursor {
            caret: 9,
            anchor: 0,
        }];

        let dark = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(dark.clear, srgb(0x27, 0x28, 0x22), "fondo Monokai");
        assert_eq!(dark.quads[0].color, Theme::dark().selection);

        editor.set_theme(Theme::light());
        let light = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(light.clear, srgb(0xFF, 0xFF, 0xFF), "fondo blanco");
        assert_eq!(light.quads[0].color, Theme::light().selection);
        assert_eq!(
            light.texts[0].color,
            Theme::light().gutter_current,
            "los números de línea usan el tema claro (línea del caret)"
        );
    }

    #[test]
    fn set_theme_recolors_syntax_tokens() {
        let mut editor = Editor::default();
        editor.lang = "rs".to_owned();
        editor.insert_text("fn x() {}");
        editor.cursors = vec![Cursor::new(0)];

        let dark_scene = editor.build_scene((600.0, 200.0), 1.0);
        let has_pink = |scene: &Scene| {
            scene
                .texts
                .iter()
                .any(|text| text.color == (0xF9, 0x26, 0x72))
        };
        assert!(has_pink(&dark_scene), "Monokai pinta `fn` de rosa");
        assert!(!editor.syntax.tokens(0).is_empty());

        editor.set_theme(Theme::light());
        assert!(
            editor.syntax.tokens(0).is_empty(),
            "cambiar de paleta invalida el resaltado"
        );
        let light_scene = editor.build_scene((600.0, 200.0), 1.0);
        assert!(!has_pink(&light_scene), "InspiredGitHub no lleva rosa");
    }

    #[test]
    fn top_inset_shifts_geometry_under_tab_bar() {
        let mut editor = Editor::default();
        editor.insert_text(&"pub fn f() {}\n".repeat(50));
        editor.set_viewport(600.0, 200.0);
        // Selección completa SIN select_all: eso no mueve el scroll (queda 0).
        editor.cursors = vec![Cursor {
            caret: editor.buffer.len_chars(),
            anchor: 0,
        }];
        // insert_text dejó el scroll abajo del todo: fijarlo a 0 para medir
        // el recorte de la barra sobre las primeras líneas.
        editor.scroll_y = 0.0;

        editor.set_top_inset(0.0);
        let plain = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(plain.quads.len(), 11, "sin barra: líneas 0..=10 visibles");

        editor.set_top_inset(32.0);
        let scene = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(
            scene.quads.len(),
            9,
            "con la barra de pestañas: sus 32px recortan ~1.6 líneas"
        );

        // El click en la línea 2 cae 32px más abajo (mismo offset que el dibujo).
        let y_line2 = 32.0 + PADDING_TOP + 2.0 * LINE_HEIGHT;
        let pos = editor.caret_at(CODE_X, y_line2, 10.0);
        assert_eq!(
            pos,
            editor.buffer.line_to_char(2),
            "caret_at compensa el inset"
        );
    }

    #[test]
    fn left_inset_shifts_horizontal_geometry() {
        let mut editor = Editor::default();
        editor.insert_text("hola");
        editor.cursors = vec![Cursor {
            caret: 4,
            anchor: 0,
        }];
        editor.scroll_y = 0.0;

        let plain = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(
            plain.quads[0].x, CODE_X,
            "sin inset la selección va al gutter"
        );

        editor.set_left_inset(500.0);
        let scene = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(
            scene.quads[0].x,
            500.0 + CODE_X,
            "la selección se desplaza con el panel"
        );
        // caret_at invierte el inset: el click a la 3ª celda del panel derecho
        // cae en la columna 2; sin inset, ese mismo x iría más allá del texto.
        assert_eq!(editor.caret_at(500.0 + CODE_X + 2.0 * 10.0, 0.0, 10.0), 2);
        editor.set_left_inset(0.0);
        assert_eq!(editor.caret_at(500.0 + CODE_X + 2.0 * 10.0, 0.0, 10.0), 4);
    }

    #[test]
    fn show_line_numbers_hides_the_gutter_and_shifts_code() {
        let mut editor = Editor::default();
        editor.insert_text("hola");
        editor.scroll_y = 0.0;

        let shown = editor.build_scene((600.0, 200.0), 1.0);
        assert!(
            shown.texts.iter().any(|span| span.text == "1"),
            "el número de línea se dibuja"
        );

        editor.set_show_line_numbers(false);
        let hidden = editor.build_scene((600.0, 200.0), 1.0);
        assert!(
            !hidden.texts.iter().any(|span| span.text == "1"),
            "el gutter desaparece"
        );
        assert_eq!(
            hidden.texts[0].x, HIDDEN_GUTTER_X,
            "el código arranca pegado a la izquierda"
        );
    }

    #[test]
    fn tab_width_setting_widens_selection_quads() {
        let mut editor = Editor::default();
        editor.insert_text("\tx");
        editor.cursors = vec![Cursor {
            caret: 2,
            anchor: 0,
        }];
        editor.scroll_y = 0.0;

        // advance = FONT_SIZE con ratio 1.0.
        let four = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(four.quads[0].w, 5.0 * FONT_SIZE, "tab de 4 celdas + la x");

        editor.set_tab_width(8);
        let eight = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(
            eight.quads[0].w,
            9.0 * FONT_SIZE,
            "el tab de 8 celdas ensancha la selección"
        );

        // 0 rompería la aritmética de columnas: se recorta a 1.
        editor.set_tab_width(0);
        let clamped = editor.build_scene((600.0, 200.0), 1.0);
        assert_eq!(clamped.quads[0].w, 2.0 * FONT_SIZE, "0 → 1 celda");
    }

    #[test]
    fn save_as_sets_path_and_writes_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nuevo.rs");
        let mut editor = Editor::default();
        editor.insert_text("fn main() {}");
        assert!(!editor.has_path());

        editor.save_as(path.clone()).expect("guarda en disco");
        assert!(editor.has_path(), "fija la ruta");
        assert_eq!(editor.tab_label(), "nuevo.rs");
        assert!(!editor.dirty(), "guardar limpia la marca de cambios");
        let on_disk = std::fs::read_to_string(&path).expect("archivo en disco");
        assert_eq!(on_disk, "fn main() {}");

        // Sin ruta, el título usa el nombre genérico.
        let blank = Editor::default();
        assert_eq!(blank.tab_label(), "sin título");
    }

    /// Bench manual: `cargo test bench_build_scene -- --ignored --nocapture`.
    #[test]
    #[ignore = "bench manual de rendimiento"]
    fn bench_build_scene() {
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
        editor.insert_text(&"pub fn f(x: u32) -> u32 { x.wrapping_add(1) } // com\n".repeat(5000));
        editor.set_viewport(1200.0, 800.0);
        let _ = editor.build_scene((1200.0, 800.0), 1.0); // primer frame: carga syntect

        let iters = 50u32;
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let _ = editor.build_scene((1200.0, 800.0), 1.0);
        }
        eprintln!("build_scene media (40 líneas): {:?}", t.elapsed() / iters);

        editor.set_viewport(1200.0, 40.0); // 2 líneas visibles
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let _ = editor.build_scene((1200.0, 40.0), 1.0);
        }
        eprintln!("build_scene media (2 líneas): {:?}", t.elapsed() / iters);
    }

    #[test]
    fn toggle_line_comment_comments_and_uncomments_selection() {
        let mut editor = Editor::default();
        editor.insert_text("hola\nadios");
        editor.select_all();
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "// hola\n// adios");
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "hola\nadios");
    }

    #[test]
    fn toggle_line_comment_keeps_indent_and_never_doubles() {
        let mut editor = Editor::default();
        editor.insert_text("    let x = 1;\n// ya\n    // también");
        editor.select_all();
        editor.toggle_line_comment();
        assert_eq!(
            text_of(&editor),
            "    // let x = 1;\n// ya\n    // también",
            "respeta la indentación y no duplica comentarios"
        );
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "    let x = 1;\nya\n    también");
    }

    #[test]
    fn toggle_line_comment_uses_hash_for_python() {
        let mut editor = Editor {
            lang: "py".to_owned(),
            ..Editor::default()
        };
        editor.insert_text("print(1)");
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "# print(1)");
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "print(1)");
    }

    #[test]
    fn toggle_line_comment_is_a_single_undo_step() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos");
        editor.select_all();
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "// uno\n// dos");
        assert!(editor.undo());
        assert_eq!(text_of(&editor), "uno\ndos");
    }

    #[test]
    fn toggle_line_comment_on_blank_line_is_reversible() {
        let mut editor = Editor::default();
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "// ");
        editor.toggle_line_comment();
        assert_eq!(text_of(&editor), "");
    }

    #[test]
    fn toggle_block_comment_wraps_line_and_unwraps() {
        let mut editor = Editor::default();
        editor.insert_text("hola");
        editor.toggle_block_comment();
        assert_eq!(text_of(&editor), "/* hola */");
        editor.toggle_block_comment();
        assert_eq!(text_of(&editor), "hola");
        assert_eq!(editor.cursors[0].caret, 4, "vuelve el caret al final");
    }

    #[test]
    fn toggle_block_comment_wraps_only_the_selection() {
        let mut editor = Editor::default();
        editor.insert_text("abc");
        editor.cursors = vec![Cursor {
            caret: 3,
            anchor: 1,
        }];
        editor.toggle_block_comment();
        assert_eq!(text_of(&editor), "a/* bc */");
        assert!(editor.undo());
        assert_eq!(text_of(&editor), "abc");
    }

    #[test]
    fn toggle_block_comment_is_a_noop_on_an_empty_line() {
        let mut editor = Editor::default();
        editor.toggle_block_comment();
        assert_eq!(text_of(&editor), "");
    }

    #[test]
    fn tab_expands_snippet_with_caret_at_zero_marker() {
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
        editor.insert_text("fn");
        let handled = editor.handle_key(&Key::Named(NamedKey::Tab), ModifiersState::empty());
        assert!(handled, "Tab consume la tecla al expandir");
        assert_eq!(text_of(&editor), "fn nombre() {\n    \n}");
        assert_eq!(editor.cursors[0].caret, 18, "caret en $0");
        assert_eq!(editor.cursors[0].anchor, 18);
        assert!(editor.undo(), "una sola transacción");
        assert_eq!(text_of(&editor), "fn");
        assert_eq!(editor.cursors[0].caret, 2);
    }

    #[test]
    fn tab_without_snippet_falls_back_to_tab_char() {
        let mut editor = Editor::default();
        editor.insert_text("hola");
        editor.handle_key(&Key::Named(NamedKey::Tab), ModifiersState::empty());
        assert_eq!(text_of(&editor), "hola\t");
    }

    #[test]
    fn snippet_skips_selection_and_multi_cursor() {
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
        editor.insert_text("fn");
        editor.cursors = vec![Cursor {
            caret: 2,
            anchor: 0,
        }];
        assert!(!editor.try_expand_snippet(), "selección → sin snippet");
        assert_eq!(text_of(&editor), "fn");

        editor.cursors = vec![
            Cursor {
                caret: 2,
                anchor: 2,
            },
            Cursor {
                caret: 0,
                anchor: 0,
            },
        ];
        assert!(!editor.try_expand_snippet(), "multi-cursor → sin snippet");
        assert_eq!(text_of(&editor), "fn");
    }
}
