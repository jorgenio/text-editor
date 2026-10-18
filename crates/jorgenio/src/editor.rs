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

/// Tamaño de fuente por defecto en px (`font_size` de settings lo cambia).
pub const FONT_SIZE: f32 = 14.0;
/// Altura de línea por defecto en px (10/7 del tamaño por defecto: 14 → 20).
pub const LINE_HEIGHT: f32 = 20.0;
/// Ancho de tabulador por defecto (el `settings.json` lo sobrescribe).
pub const TAB_WIDTH: usize = 4;

/// Altura de línea en px para un tamaño de fuente dado, con la misma
/// proporción que los defaults (14 px → 20 px exactos).
pub fn line_height_for(font_size: f32) -> f32 {
    font_size * LINE_HEIGHT / FONT_SIZE
}

/// Inicio del área de código (tras el gutter).
const CODE_X: f32 = 64.0;
/// Inicio del código con el gutter oculto (`show_line_numbers: false`).
const HIDDEN_GUTTER_X: f32 = 12.0;
/// Derecha de los números de línea (alineados a la derecha).
const GUTTER_RIGHT_X: f32 = 56.0;
const PADDING_TOP: f32 = 8.0;
const CARET_WIDTH: f32 = 2.0;

/// Advance por defecto (celdas por emisión), el mismo fallback que usa
/// `probe_advance_ratio` en jorgenio-ui; `build_scene` lo refina con el
/// valor real medido en la fuente del sistema.
const DEFAULT_ADVANCE_RATIO: f32 = 0.6;
/// Holgura en px a la derecha de cada fila envuelta: evita que un glifo
/// redondeado se recorte justo en el borde del panel.
const WRAP_PAD: f32 = 8.0;

/// Caché de envoltura de líneas (solo con [`Editor::word_wrap`] activo).
///
/// `prefix[k]` es el número de filas de pantalla que ocupan las líneas
/// lógicas `0..k`: tiene `line_count + 1` elementos, empieza en 0 y crece
/// al menos en 1 por línea, de modo que `rows_of_line(k) = prefix[k+1] -
/// prefix[k]` y `total_rows = *prefix.last()`. Se reconstruye entera cuando
/// cambia el buffer, el ancho disponible, la fuente o el tabulador (todas
/// claves de [`WrapCache::is_stale`]).
#[derive(Debug, Clone, Default, PartialEq)]
struct WrapCache {
    /// Celdas de pantalla por fila con las que se calculó.
    wrap_cells: usize,
    /// Ancho de tabulador con el que se calculó.
    tab_width: usize,
    /// Revisión del buffer con la que se calculó.
    revision: u64,
    /// Acumulado de filas por línea (ver la descripción del tipo).
    prefix: Vec<u32>,
}

impl WrapCache {
    /// ¿Ha cambiado algo relevante desde el último cálculo?
    fn is_stale(&self, wrap_cells: usize, tab_width: usize, revision: u64) -> bool {
        self.wrap_cells != wrap_cells || self.tab_width != tab_width || self.revision != revision
    }

    /// Cuenta cuántas filas de pantalla ocupa cada línea de `buffer`.
    fn build(buffer: &Buffer, wrap_cells: usize, tab_width: usize) -> Self {
        let wrap_cells = wrap_cells.max(1);
        let mut prefix = Vec::with_capacity(buffer.line_count() + 1);
        prefix.push(0u32);
        let mut rows = 0u32;
        for line in 0..buffer.line_count() {
            let text = buffer.line_text(line);
            let cells = movement::visual_column(&text, text.chars().count(), tab_width);
            rows += cells.div_ceil(wrap_cells).max(1) as u32;
            prefix.push(rows);
        }
        Self {
            wrap_cells,
            tab_width,
            revision: buffer.revision(),
            prefix,
        }
    }

    /// Filas que ocupa la línea lógica `line` (≥1; fuera de rango → 1).
    fn rows_of_line(&self, line: usize) -> usize {
        let before = self.prefix.get(line).copied().unwrap_or(0);
        let after = self
            .prefix
            .get(line + 1)
            .copied()
            .unwrap_or(before.saturating_add(1));
        (after - before).max(1) as usize
    }

    /// Primera fila de pantalla de la línea lógica `line`.
    fn rows_before(&self, line: usize) -> usize {
        match self.prefix.get(line) {
            Some(&rows) => rows as usize,
            // Más allá del final: la última fila conocida.
            None => self.total_rows().saturating_sub(1),
        }
    }

    /// Filas de pantalla de todo el buffer (≥1).
    fn total_rows(&self) -> usize {
        self.prefix.last().copied().unwrap_or(1).max(1) as usize
    }

    /// Línea lógica que ocupa la fila de pantalla `row` (recortada al final).
    fn line_at_row(&self, row: usize) -> usize {
        if self.prefix.len() < 2 {
            return 0;
        }
        // Primer acumulado estrictamente mayor que `row`, menos uno.
        let idx = self.prefix.partition_point(|&cum| (cum as usize) <= row);
        idx.saturating_sub(1).min(self.prefix.len() - 2)
    }
}

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
    /// ¿Tabulador escribe espacios? (`translate_tabs_to_spaces` de prefs).
    translate_tabs_to_spaces: bool,
    /// ¿Auto-cerrar pares de paréntesis/comillas? (`auto_match` de prefs).
    auto_match: bool,
    /// Tamaño de la fuente en px (preferencias; ver [`FONT_SIZE`]). El
    /// advance, la altura de línea y el gutter se derivan de él.
    font_size: f32,
    /// Tema de colores activo (oscuro Monokai / claro); lo fija `main`.
    theme: Theme,
    /// Buscar (Ctrl+F): rangos de `find_query` en orden, y el actual.
    find: Vec<Range<usize>>,
    find_current: usize,
    find_query: String,
    /// Revisión del buffer con la que `find` está calculado.
    find_revision: u64,
    /// ¿Buscar por regex? (botón ".*" de la barra; M23). La consulta se
    /// compila al escribirla: si el patrón es inválido no hay coincidencias
    /// y [`Editor::find_regex_invalid`] lo dice para la barra.
    find_regex: bool,
    find_regex_compiled: Option<fancy_regex::Regex>,
    find_regex_error: bool,
    /// ¿Ajustar líneas largas al ancho del panel? (`word_wrap` de prefs).
    /// Al activarse cambia la geometría (una línea lógica = varias filas
    /// de pantalla); el buffer NO se toca.
    word_wrap: bool,
    /// Advance en celdas del carácter (px por celda). Se actualiza en cada
    /// `build_scene` y lo usan scroll/caret para saber cuántas celdas caben
    /// por fila sin esperar al siguiente frame.
    advance_ratio: f32,
    /// Caché de envoltura (solo con `word_wrap`): nº de celdas que caben
    /// por fila y cuántas filas ocupa cada línea lógica. Se reconstruye con
    /// `ensure_wrap` cuando cambia buffer/ancho/fuente/indentación.
    wrap: WrapCache,
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
            translate_tabs_to_spaces: false,
            auto_match: true,
            font_size: FONT_SIZE,
            theme: Theme::dark(),
            find: Vec::new(),
            find_current: 0,
            find_query: String::new(),
            find_revision: 0,
            find_regex: false,
            find_regex_compiled: None,
            find_regex_error: false,
            word_wrap: false,
            advance_ratio: DEFAULT_ADVANCE_RATIO,
            wrap: WrapCache::default(),
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

    /// Reconstruye un buffer desde el hot exit de la sesión (M23): `text` es
    /// lo que quedó sin guardar (puede no coincidir con el disco) y el
    /// buffer arranca como sucio, para que el "●" y Ctrl+S sigan honestos.
    pub fn from_hot(path: Option<PathBuf>, text: &str) -> Self {
        let mut editor = match path {
            Some(path) => Self {
                lang: SyntaxHighlighter::detect(&path),
                path: Some(path),
                ..Self::default()
            },
            None => Self::default(),
        };
        editor.buffer = Buffer::from_text(text);
        editor.buffer.set_dirty();
        editor
    }

    /// Texto completo del buffer (para el hot exit de la sesión).
    pub fn text(&self) -> String {
        self.buffer.text_range(0..self.buffer.len_chars())
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

    pub fn set_translate_tabs_to_spaces(&mut self, translate: bool) {
        self.translate_tabs_to_spaces = translate;
    }

    pub fn set_auto_match(&mut self, on: bool) {
        self.auto_match = on;
    }

    /// Activa o desactiva el ajuste de línea (`word_wrap` de preferencias).
    /// El buffer no se toca: solo la geometría (una línea lógica pasa a
    /// ocupar varias filas de pantalla) y, con ella, el tope del scroll.
    pub fn set_word_wrap(&mut self, on: bool) {
        self.word_wrap = on;
        self.ensure_wrap();
        self.clamp_scroll();
    }

    /// Tamaño de la fuente en px (recortado a 8-72). La altura de línea,
    /// el advance del código y el ancho del gutter se derivan solos.
    pub fn set_font_size(&mut self, px: u32) {
        self.font_size = px.clamp(8, 72) as f32;
    }

    /// Altura de línea actual en px (derivada del tamaño de la fuente).
    fn line_height(&self) -> f32 {
        line_height_for(self.font_size)
    }

    /// Proporción de tamaño frente al default (1.0 = sin cambiar).
    fn font_ratio(&self) -> f32 {
        self.font_size / FONT_SIZE
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

    /// Inicio del área de código según el gutter (preferencias); escala
    /// con el tamaño de fuente para que los números no pisen el código.
    fn code_offset(&self) -> f32 {
        if self.show_line_numbers {
            CODE_X * self.font_ratio()
        } else {
            HIDDEN_GUTTER_X
        }
    }

    /// Derecha de los números de línea (alineados hacia ella); escala con
    /// el tamaño de fuente (14 px → 56 px exactos).
    fn gutter_right(&self) -> f32 {
        GUTTER_RIGHT_X * self.font_ratio()
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

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn translate_tabs_to_spaces(&self) -> bool {
        self.translate_tabs_to_spaces
    }

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn auto_match(&self) -> bool {
        self.auto_match
    }

    /// Solo para tests (comprobar `apply_prefs`).
    #[cfg(test)]
    pub fn word_wrap(&self) -> bool {
        self.word_wrap
    }

    /// Mueve el caret a `line` (1-based, como el prompt de "Ir a la línea…")
    /// y lo deja visible. Líneas fuera de rango → la última.
    pub fn goto_line(&mut self, line: usize) {
        let last = self.buffer.char_to_line(self.buffer.len_chars());
        let line = line.saturating_sub(1).min(last);
        let at = self.buffer.line_to_char(line);
        self.cursors = vec![Cursor::new(at)];
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Texto de la barra de estado: Ln/Col (columna visual), selección,
    /// tabulado, lenguaje y nombre del archivo.
    pub fn status_text(&self) -> String {
        let caret = self.primary().caret;
        let line = self.buffer.char_to_line(caret);
        let line_start = self.buffer.line_to_char(line);
        let col = movement::visual_column(
            &self.buffer.line_text(line),
            caret - line_start,
            self.tab_width,
        ) + 1;
        let mut parts = vec![format!("Ln {}, Col {}", line + 1, col)];
        if let Some(sel) = self.selection_status() {
            parts.push(sel);
        }
        parts.push(if self.translate_tabs_to_spaces {
            format!("Esp: {}", self.tab_width)
        } else {
            format!("Tab: {}", self.tab_width)
        });
        parts.push(self.lang_label());
        parts.push(self.tab_label());
        parts.join("  |  ")
    }

    /// "N líneas, M chars" de la selección del cursor principal (si la hay).
    fn selection_status(&self) -> Option<String> {
        let range = self.primary().range();
        if range.is_empty() {
            return None;
        }
        let first = self.buffer.char_to_line(range.start);
        let last = self.buffer.char_to_line(range.end.max(range.start + 1) - 1);
        Some(format!(
            "{} líneas, {} chars",
            last - first + 1,
            range.end - range.start
        ))
    }

    /// Nombre mostrable del lenguaje según la extensión detectada.
    fn lang_label(&self) -> String {
        match self.lang.as_str() {
            "" => "Texto plano".to_owned(),
            "rs" => "Rust".to_owned(),
            "py" => "Python".to_owned(),
            "js" | "mjs" | "cjs" => "JavaScript".to_owned(),
            "ts" => "TypeScript".to_owned(),
            "json" => "JSON".to_owned(),
            "md" => "Markdown".to_owned(),
            "toml" => "TOML".to_owned(),
            "yml" | "yaml" => "YAML".to_owned(),
            "sh" | "bash" => "Shell".to_owned(),
            "html" | "htm" => "HTML".to_owned(),
            "css" => "CSS".to_owned(),
            "c" | "h" => "C".to_owned(),
            "cpp" | "cc" | "cxx" | "hpp" => "C++".to_owned(),
            other => other.to_uppercase(),
        }
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
        let ctrl = modifiers.control_key() || modifiers.super_key();
        let alt = modifiers.alt_key();

        let handled = match key {
            Key::Named(NamedKey::Backspace) if ctrl => {
                self.delete_word_left();
                true
            }
            Key::Named(NamedKey::Delete) if ctrl => {
                self.delete_word_right();
                true
            }
            Key::Named(NamedKey::Backspace) => {
                self.backspace();
                true
            }
            Key::Named(NamedKey::Delete) => {
                self.delete_forward();
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.insert_newline_indented();
                true
            }
            Key::Named(NamedKey::Tab) if shift => {
                self.unindent_selection();
                true
            }
            Key::Named(NamedKey::Tab) => {
                if self.try_expand_snippet() {
                    true
                } else if self.cursors.iter().any(|cursor| cursor.has_selection()) {
                    self.indent_selection();
                    true
                } else {
                    let unit = self.indent_unit();
                    self.insert_text(&unit);
                    true
                }
            }
            Key::Named(NamedKey::Escape) => {
                self.collapse_to_primary();
                true
            }
            Key::Named(NamedKey::ArrowLeft) if ctrl && !alt => {
                self.move_cursors(
                    |cursor, buffer| movement::word_left(buffer, cursor.caret),
                    shift,
                );
                true
            }
            Key::Named(NamedKey::ArrowRight) if ctrl && !alt => {
                self.move_cursors(
                    |cursor, buffer| movement::word_right(buffer, cursor.caret),
                    shift,
                );
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
                self.vertical_step(-1, shift);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.vertical_step(1, shift);
                true
            }
            Key::Named(NamedKey::Home) => {
                self.smart_home(shift);
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
                self.vertical_step(page, shift);
                true
            }
            Key::Named(NamedKey::PageDown) => {
                let page = self.page_lines();
                self.vertical_step(page, shift);
                true
            }
            Key::Character(text) => {
                let mut chars = text.chars();
                let first = chars.next();
                if let (Some(ch), None) = (first, chars.next())
                    && self.auto_match_single(ch)
                {
                    // Par insertado o skip-over del cierre.
                } else {
                    self.insert_text(text);
                }
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
                    // Entre un par de auto_match: borra los dos lados.
                    let caret = range.start;
                    let len = editor.buffer.len_chars();
                    let prev = editor.buffer.text_range(caret - 1..caret);
                    let next = if caret < len {
                        editor.buffer.text_range(caret..caret + 1)
                    } else {
                        String::new()
                    };
                    if editor.auto_match && editor.cursors.len() == 1 && is_auto_pair(&prev, &next)
                    {
                        (caret - 1, caret + 1)
                    } else {
                        (movement::prev_grapheme(&editor.buffer, caret), caret)
                    }
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

    /// Escribe el par automático o salta por delante del cierre ya
    /// presente (como Sublime con `auto_match`): solo con UN cursor sin
    /// selección. `true` = el carácter ya está gestionado.
    fn auto_match_single(&mut self, ch: char) -> bool {
        if !self.auto_match || self.cursors.len() != 1 || self.cursors[0].has_selection() {
            return false;
        }
        let caret = self.cursors[0].caret;
        let len = self.buffer.len_chars();
        let next = if caret < len {
            self.buffer.text_range(caret..caret + 1)
        } else {
            String::new()
        };
        if let Some(closer) = auto_closer_of(ch) {
            if next == closer.to_string() {
                // Ya está el cierre: solo avanza el caret.
                self.cursors[0].caret = caret + 1;
                self.cursors[0].anchor = caret + 1;
                return true;
            }
            self.insert_text(&format!("{ch}{closer}"));
            let pos = caret + 1;
            self.cursors[0].caret = pos;
            self.cursors[0].anchor = pos;
            return true;
        }
        // Tipear el cierre directamente antes del existente también salta.
        if matches!(ch, ')' | ']' | '}') && next == ch.to_string() {
            self.cursors[0].caret = caret + 1;
            self.cursors[0].anchor = caret + 1;
            return true;
        }
        false
    }

    /// Ctrl+Backspace: borra hacia atrás hasta el inicio de la palabra
    /// (con selección se comporta como Backspace normal).
    pub fn delete_word_left(&mut self) {
        if self.cursors.iter().any(|cursor| cursor.has_selection()) {
            self.backspace();
            return;
        }
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let caret = editor.cursors[i].caret;
                let start = movement::word_left(&editor.buffer, caret);
                if start == caret {
                    continue;
                }
                editor.buffer.delete(start..caret);
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, start, caret, 0);
                    other.anchor = shift_after(other.anchor, start, caret, 0);
                }
                editor.cursors[i].caret = start;
                editor.cursors[i].anchor = start;
            }
        });
        self.reset_caret_blink();
    }

    /// Ctrl+Delete: borra hacia delante hasta el inicio de la palabra
    /// siguiente (con selección se comporta como Delete normal).
    pub fn delete_word_right(&mut self) {
        if self.cursors.iter().any(|cursor| cursor.has_selection()) {
            self.delete_forward();
            return;
        }
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let caret = editor.cursors[i].caret;
                let end = movement::word_right(&editor.buffer, caret);
                if end == caret {
                    continue;
                }
                editor.buffer.delete(caret..end);
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, caret, end, 0);
                    other.anchor = shift_after(other.anchor, caret, end, 0);
                }
                editor.cursors[i].caret = caret;
                editor.cursors[i].anchor = caret;
            }
        });
        self.reset_caret_blink();
    }

    /// Home inteligente (Sublime): alterna entre la primera columna y el
    /// primer carácter no-blanco de la línea.
    fn smart_home(&mut self, shift: bool) {
        self.move_cursors(
            |cursor, buffer| {
                let line = buffer.char_to_line(cursor.caret);
                let line_start = buffer.line_to_char(line);
                let first = buffer
                    .line_text(line)
                    .chars()
                    .take_while(|c| c.is_whitespace() && *c != '\n')
                    .count();
                let target = line_start + first;
                if cursor.caret == target {
                    line_start
                } else {
                    target
                }
            },
            shift,
        );
    }

    /// Unidad de indentación: `\t` o `tab_width` espacios.
    fn indent_unit(&self) -> String {
        if self.translate_tabs_to_spaces {
            " ".repeat(self.tab_width)
        } else {
            "\t".to_owned()
        }
    }

    /// Líneas únicas (sin repetir) que abarcan las selecciones actuales; una
    /// selección que termina justo al inicio de una línea no la incluye.
    fn selected_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        for cursor in &self.cursors {
            let range = cursor.range();
            let first = self.buffer.char_to_line(range.start);
            let mut last = self.buffer.char_to_line(if range.end > range.start {
                range.end - 1
            } else {
                range.start
            });
            if last > first && range.end == self.buffer.line_to_char(last) {
                last -= 1;
            }
            lines.extend(first..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// Indenta (Tab con selección) cada línea abarcada, sin cambiar la
    /// columna del texto.
    pub fn indent_selection(&mut self) {
        let lines = self.selected_lines();
        let unit = self.indent_unit();
        self.with_transaction(|editor| {
            for &line in lines.iter().rev() {
                let at = editor.buffer.line_to_char(line);
                let len = unit.chars().count();
                editor.buffer.insert(at, &unit);
                for cursor in &mut editor.cursors {
                    cursor.caret = shift_after(cursor.caret, at, at, len);
                    cursor.anchor = shift_after(cursor.anchor, at, at, len);
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Desindenta (Shift+Tab) quitando hasta un nivel de espacios o un `\t`.
    pub fn unindent_selection(&mut self) {
        let lines = self.selected_lines();
        let width = self.tab_width;
        self.with_transaction(|editor| {
            for &line in lines.iter().rev() {
                let at = editor.buffer.line_to_char(line);
                let text = editor.buffer.line_text(line);
                let ws = text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                if ws == 0 {
                    continue;
                }
                let remove = if text.starts_with('\t') {
                    1
                } else {
                    ws.min(width)
                };
                editor.buffer.delete(at..at + remove);
                for cursor in &mut editor.cursors {
                    cursor.caret = shift_after(cursor.caret, at, at + remove, 0);
                    cursor.anchor = shift_after(cursor.anchor, at, at + remove, 0);
                }
            }
        });
        self.reset_caret_blink();
    }

    /// Ctrl+L: selección a la línea completa (repetir extiende a la
    /// siguiente); el `\n` final queda dentro de la selección.
    pub fn expand_to_line(&mut self) {
        let len = self.buffer.len_chars();
        let mut cursors = Vec::with_capacity(self.cursors.len());
        for cursor in &self.cursors {
            let range = cursor.range();
            let (start, end) = (range.start, range.end);
            let line = self.buffer.char_to_line(start);
            let line_start = self.buffer.line_to_char(line);
            let end_line = self
                .buffer
                .char_to_line(if end > start { end - 1 } else { start });
            let end_line_start = self.buffer.line_to_char(end_line);
            let end_line_chars = self.buffer.line_text(end_line).chars().count();
            let end_line_full = (end_line_start + end_line_chars + 1).min(len);
            let caret = if end == end_line_full && end < len {
                // Ya cubre la línea entera: extiende a la siguiente.
                let next_chars = self.buffer.line_text(end_line + 1).chars().count();
                (end + next_chars + 1).min(len)
            } else {
                end_line_full
            };
            cursors.push(Cursor {
                anchor: line_start,
                caret,
            });
        }
        self.cursors = cursors;
        self.sort_and_merge();
        self.reset_caret_blink();
        self.scroll_into_view();
    }

    /// Ctrl+J: une la línea actual con la siguiente (quita el `\n` y el
    /// sangrado de la siguiente; pone un espacio si ambas tienen texto).
    pub fn join_lines(&mut self) {
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let caret = editor.cursors[i].caret;
                let len = editor.buffer.len_chars();
                let line = editor.buffer.char_to_line(caret);
                let line_start = editor.buffer.line_to_char(line);
                let cur_chars = editor.buffer.line_text(line).chars().count();
                let nl = line_start + cur_chars;
                if nl >= len {
                    continue; // última línea sin \n: no hay nada que unir
                }
                let next_start = editor.buffer.line_to_char(line + 1);
                let next_text = editor.buffer.line_text(line + 1);
                let next_chars = next_text.chars().count();
                let next_indent = next_text
                    .chars()
                    .take_while(|c| c.is_whitespace() && *c != '\n')
                    .count();
                let end = if next_indent == next_chars {
                    next_start + next_chars
                } else {
                    next_start + next_indent
                };
                let cur_text = editor.buffer.line_text(line);
                let cur_blank =
                    cur_chars == 0 || cur_text.chars().last().is_some_and(|c| c.is_whitespace());
                let next_blank = next_indent == next_chars;
                let space = if cur_blank || next_blank { "" } else { " " };
                let inserted = space.chars().count();
                editor.buffer.delete(nl..end);
                if inserted > 0 {
                    editor.buffer.insert(nl, space);
                }
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, nl, end, inserted);
                    other.anchor = shift_after(other.anchor, nl, end, inserted);
                }
                editor.cursors[i].caret = nl + inserted;
                editor.cursors[i].anchor = nl + inserted;
            }
        });
        self.reset_caret_blink();
    }

    /// Ctrl+Shift+↑/↓: mueve la línea actual intercambiándola con la
    /// contigua; el caret conserva su columna dentro de la línea.
    pub fn move_line_up(&mut self) {
        self.move_line(-1);
    }

    pub fn move_line_down(&mut self) {
        self.move_line(1);
    }

    fn move_line(&mut self, dir: i32) {
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let caret = editor.cursors[i].caret;
                let line = editor.buffer.char_to_line(caret);
                let col = caret - editor.buffer.line_to_char(line);
                let last_line = editor.buffer.char_to_line(editor.buffer.len_chars());
                let target = if dir < 0 {
                    line.checked_sub(1)
                } else if line < last_line {
                    Some(line + 1)
                } else {
                    None
                };
                let Some(target) = target else { continue };
                let (low_line, high_line) = if dir < 0 {
                    (target, line)
                } else {
                    (line, target)
                };
                let low_start = editor.buffer.line_to_char(low_line);
                let high_start = editor.buffer.line_to_char(high_line);
                let low_text = editor.buffer.line_text(low_line).to_owned();
                let high_text = editor.buffer.line_text(high_line).to_owned();
                let high_chars = high_text.chars().count();
                let has_nl = high_start + high_chars < editor.buffer.len_chars();
                let block_end = high_start + high_chars + usize::from(has_nl);
                let new_block = if has_nl {
                    format!("{high_text}\n{low_text}\n")
                } else {
                    format!("{high_text}\n{low_text}")
                };
                let new_len = new_block.chars().count();
                editor.buffer.delete(low_start..block_end);
                editor.buffer.insert(low_start, &new_block);
                let new_line_start = if dir < 0 {
                    low_start
                } else {
                    low_start + high_chars + 1
                };
                let new_caret = new_line_start + col;
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, low_start, block_end, new_len);
                    other.anchor = shift_after(other.anchor, low_start, block_end, new_len);
                }
                editor.cursors[i].caret = new_caret;
                editor.cursors[i].anchor = new_caret;
            }
        });
        self.reset_caret_blink();
    }

    /// Enter: además del `\n`, arrastra el sangrado de la línea actual.
    fn insert_newline_indented(&mut self) {
        if self.cursors.iter().any(|cursor| cursor.has_selection()) {
            self.insert_text("\n");
            return;
        }
        self.with_transaction(|editor| {
            let mut i = editor.cursors.len();
            while i > 0 {
                i -= 1;
                let caret = editor.cursors[i].caret;
                let line = editor.buffer.char_to_line(caret);
                let indent: String = editor
                    .buffer
                    .line_text(line)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let text = format!("\n{indent}");
                let len = text.chars().count();
                editor.buffer.insert(caret, &text);
                for other in &mut editor.cursors[i + 1..] {
                    other.caret = shift_after(other.caret, caret, caret, len);
                    other.anchor = shift_after(other.anchor, caret, caret, len);
                }
                editor.cursors[i].caret = caret + len;
                editor.cursors[i].anchor = caret + len;
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
        self.ensure_wrap();
        // Con envoltura el paso es de fila de pantalla; sin ella, de línea.
        let wrap = self.word_wrap.then(|| self.wrap.clone());
        let tab_width = self.tab_width;
        let mut additions = Vec::with_capacity(self.cursors.len());
        for cursor in &self.cursors {
            let target = if let Some(wrap) = &wrap {
                wrap_vertical_move(&self.buffer, cursor.caret, delta, wrap, tab_width)
            } else {
                let line = self.buffer.char_to_line(cursor.caret);
                let line_start = self.buffer.line_to_char(line);
                let text = self.buffer.line_text(line);
                let col = cursor.caret.saturating_sub(line_start);
                let visual = movement::visual_column(&text, col, tab_width);
                let target_line = (line as isize + delta)
                    .clamp(0, self.buffer.line_count() as isize - 1)
                    as usize;
                let target_text = self.buffer.line_text(target_line);
                let target_col = movement::char_col_at_visual(&target_text, visual, tab_width);
                self.buffer.line_to_char(target_line) + target_col
            };
            additions.push(Cursor::new(target));
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
        if self.find_regex {
            self.recompile_find_regex();
        }
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

    /// Activa/desactiva la búsqueda por regex (botón ".*" de la barra; M23)
    /// y recalcula las coincidencias con la consulta actual.
    pub fn set_find_regex(&mut self, on: bool) {
        if self.find_regex == on {
            return;
        }
        self.find_regex = on;
        self.recompile_find_regex();
        if self.find_query.is_empty() {
            self.find.clear();
            self.find_current = 0;
            self.find_revision = self.buffer.revision();
            return;
        }
        let from = self.primary().caret;
        self.recompute_find(from);
        self.reveal_current_match();
    }

    /// ¿Modo regex activo con un patrón inválido? (lo enseña la barra en
    /// vez del contador "0 / 0", que no distingue "no coincide" de "no
    /// compila").
    pub fn find_regex_invalid(&self) -> bool {
        self.find_regex && self.find_regex_error
    }

    /// Compila `find_query` en modo regex. smart-case: mayúsculas en el
    /// patrón → distingue mayúsculas; sin ellas, plegado insensible (en
    /// regex no hay plegado ASCII parcial: se pliega entero). `^`/`$` marcan
    /// límites de línea (multi_line), que es lo que se espera al buscar en
    /// código. Un patrón inválido no compila: no hay coincidencias.
    fn recompile_find_regex(&mut self) {
        if !self.find_regex || self.find_query.is_empty() {
            self.find_regex_compiled = None;
            self.find_regex_error = false;
            return;
        }
        let case_insensitive = !self.find_query.chars().any(char::is_uppercase);
        match fancy_regex::RegexBuilder::new(&self.find_query)
            .case_insensitive(case_insensitive)
            .multi_line(true)
            .build()
        {
            Ok(regex) => {
                self.find_regex_compiled = Some(regex);
                self.find_regex_error = false;
            }
            Err(err) => {
                log::debug!("regex inválida ({err}): {}", self.find_query);
                self.find_regex_compiled = None;
                self.find_regex_error = true;
            }
        }
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
        self.find = if self.find_regex {
            self.find_regex_compiled
                .as_ref()
                .map_or_else(Vec::new, |regex| find_in_buffer_regex(&self.buffer, regex))
        } else {
            find_in_buffer(&self.buffer, &self.find_query)
        };
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
        self.find_regex_compiled = None;
        self.find_regex_error = false;
        self.find_revision = self.buffer.revision();
        self.reset_caret_blink();
    }

    /// Reemplaza la coincidencia actual en su transacción de undo y avanza a
    /// la siguiente a partir del texto nuevo (con vuelta). `false` si no hay.
    /// En modo regex el reemplazo expande `$1`/`$name` con los captures de
    /// la propia coincidencia.
    pub fn find_replace_current(&mut self, replacement: &str) -> bool {
        self.find_sync();
        let Some(range) = self.find.get(self.find_current).cloned() else {
            return false;
        };
        let start = range.start;
        let expanded = if self.find_regex {
            match &self.find_regex_compiled {
                Some(regex) => {
                    let hay = self.buffer.text_range(0..self.buffer.len_chars());
                    expand_regex_replacement(regex, &hay, &range, replacement)
                }
                None => replacement.to_owned(),
            }
        } else {
            replacement.to_owned()
        };
        let inserted = expanded.chars().count();
        self.with_transaction(|editor| editor.replace_match(&range, &expanded));
        self.recompute_find(start + inserted);
        self.reveal_current_match();
        true
    }

    /// Reemplaza TODAS las coincidencias de derecha a izquierda (los offsets
    /// altos no afectan a los bajos) en UNA transacción de undo.
    /// Devuelve cuántas había. En modo regex cada reemplazo se expande con
    /// los captures de SU coincidencia, calculados sobre el texto original
    /// (los reemplazos de la derecha no afectan a la izquierda).
    pub fn find_replace_all(&mut self, replacement: &str) -> usize {
        self.find_sync();
        if self.find.is_empty() {
            return 0;
        }
        let count = self.find.len();
        if self.find_regex && self.find_regex_compiled.is_some() {
            let hay = self.buffer.text_range(0..self.buffer.len_chars());
            let mut todo: Vec<(Range<usize>, String)> = Vec::new();
            if let Some(regex) = &self.find_regex_compiled {
                for caps in regex.captures_iter(&hay) {
                    let Ok(caps) = caps else { break };
                    let Some(m) = caps.get(0) else { continue };
                    if m.start() == m.end() {
                        continue;
                    }
                    let start = hay[..m.start()].chars().count();
                    let len = hay[m.start()..m.end()].chars().count();
                    let mut expanded = String::new();
                    caps.expand(replacement, &mut expanded);
                    todo.push((start..start + len, expanded));
                }
            }
            self.with_transaction(|editor| {
                for (range, expanded) in todo.iter().rev() {
                    editor.replace_match(range, expanded);
                }
            });
            self.recompute_find(0);
            self.reset_caret_blink();
            return todo.len();
        }
        let matches = self.find.clone();
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
        // El ratio medido en la fuente manda también fuera de este frame:
        // la caché de envoltura (y por tanto el scroll y el caret) depende
        // de cuántas celdas caben por fila.
        self.advance_ratio = advance_ratio;
        self.ensure_wrap();
        let bg = self.theme.editor_bg;
        let mut scene = Scene::new(srgb(bg.0, bg.1, bg.2));
        if self.word_wrap {
            self.build_wrapped(&mut scene, viewport, advance_ratio);
            return scene;
        }
        let advance = self.font_size * advance_ratio;
        let line_h = self.line_height();
        let font = self.font_size;
        let line_count = self.buffer.line_count();
        let inset = self.top_inset;
        let xin = self.left_inset;
        let cx = xin + self.code_offset();
        let gutter_right = xin + self.gutter_right();
        let tab_width = self.tab_width;

        let visible_first = (((self.scroll_y - inset - PADDING_TOP) / line_h)
            .floor()
            .max(0.0) as usize)
            .min(line_count - 1);
        let visible_last = (((self.scroll_y + viewport.1 - inset - PADDING_TOP) / line_h).ceil()
            as usize)
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
            line_height: line_h,
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
            let y = inset + PADDING_TOP + line as f32 * line_h - self.scroll_y;
            if self.show_line_numbers {
                let color = if line == caret_line {
                    self.theme.gutter_current
                } else {
                    self.theme.gutter_fg
                };
                let number = format!("{}", line + 1);
                let number_x = gutter_right - number.len() as f32 * advance;
                scene
                    .texts
                    .push(TextSpan::new(number, number_x, y, font, color));
            }

            let text = self.buffer.line_text(line);
            if text.is_empty() {
                continue;
            }
            let tokens = self.syntax.tokens(line);
            if tokens.is_empty() {
                scene
                    .texts
                    .push(TextSpan::new(text, cx, y, font, self.theme.fg));
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
                    font,
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
                let y = inset + PADDING_TOP + line as f32 * line_h - self.scroll_y;
                let col = cursor.caret - self.buffer.line_to_char(line);
                let cells = movement::visual_column(&self.buffer.line_text(line), col, tab_width);
                scene.overlays.push(Quad::new(
                    cx + cells as f32 * advance,
                    y,
                    CARET_WIDTH,
                    line_h - 2.0,
                    self.theme.caret,
                ));
            }
        }
        scene
    }

    /// Rama de [`Editor::build_scene`] con `word_wrap` activo: recorre FILAS
    /// de pantalla en vez de líneas lógicas, de modo que una línea larga
    /// pinta varias filas y el número de línea solo aparece en la primera.
    fn build_wrapped(&mut self, scene: &mut Scene, viewport: (f32, f32), advance_ratio: f32) {
        let advance = self.font_size * advance_ratio;
        let line_h = self.line_height();
        let font = self.font_size;
        let inset = self.top_inset;
        let cx = self.left_inset + self.code_offset();
        let gutter_right = self.left_inset + self.gutter_right();
        let tab_width = self.tab_width;
        let per_row = self.wrap.wrap_cells.max(1);

        let (first_row, last_row) = self.visible_rows(viewport.1);
        let last_line = self.wrap.line_at_row(last_row);

        self.syntax.set_extension(&self.lang);
        self.syntax.prepare(&mut self.buffer, last_line);
        self.find_sync();

        for cursor in &self.cursors {
            let range = cursor.range();
            self.push_wrapped_range(scene, &range, self.theme.selection, viewport.1);
        }
        for (index, range) in self.find.iter().enumerate() {
            let color = if index == self.find_current {
                self.theme.current_match
            } else {
                self.theme.match_bg
            };
            self.push_wrapped_range(scene, range, color, viewport.1);
        }

        let caret_line = self.buffer.char_to_line(self.primary().caret);
        for row in first_row..=last_row {
            let y = inset + PADDING_TOP + row as f32 * line_h - self.scroll_y;
            let line = self.wrap.line_at_row(row);
            let row_in_line = row - self.wrap.rows_before(line);
            let cells_from = row_in_line * per_row;
            let cells_to = cells_from + per_row;

            if row_in_line == 0 && self.show_line_numbers {
                let color = if line == caret_line {
                    self.theme.gutter_current
                } else {
                    self.theme.gutter_fg
                };
                let number = format!("{}", line + 1);
                let number_x = gutter_right - number.len() as f32 * advance;
                scene
                    .texts
                    .push(TextSpan::new(number, number_x, y, font, color));
            }

            let text = self.buffer.line_text(line);
            if text.is_empty() {
                continue;
            }
            let tokens = self.syntax.tokens(line);
            if tokens.is_empty() {
                let piece = row_slice(&text, cells_from, cells_to, tab_width);
                if !piece.is_empty() {
                    scene
                        .texts
                        .push(TextSpan::new(piece, cx, y, font, self.theme.fg));
                }
                continue;
            }
            // Columnas de todos los tokens de la línea en dos recorridos
            // (uno de inicios, otro de fines) y recorte a la fila actual.
            let starts: Vec<usize> = tokens.iter().map(|token| token.start).collect();
            let ends: Vec<usize> = tokens.iter().map(|token| token.end).collect();
            let start_cells = movement::visual_columns_at(&text, &starts, tab_width);
            let end_cells = movement::visual_columns_at(&text, &ends, tab_width);
            for (token, (&start, &end)) in tokens.iter().zip(start_cells.iter().zip(&end_cells)) {
                let cell_from = start.max(cells_from);
                let cell_to = end.min(cells_to);
                if cell_from >= cell_to {
                    continue;
                }
                let piece = row_slice(&text, cell_from, cell_to, tab_width);
                if piece.is_empty() || piece.chars().all(|c| c == '\n') {
                    continue;
                }
                scene.texts.push(TextSpan::new(
                    piece,
                    cx + (cell_from - cells_from) as f32 * advance,
                    y,
                    font,
                    token.color,
                ));
            }
        }

        if self.focused && self.caret_on {
            for cursor in &self.cursors {
                let line = self.buffer.char_to_line(cursor.caret);
                let text = self.buffer.line_text(line);
                let col = cursor.caret - self.buffer.line_to_char(line);
                let cells = movement::visual_column(&text, col, tab_width);
                let row = self.row_of_line_cells(line, cells);
                if row < first_row || row > last_row {
                    continue;
                }
                let y = inset + PADDING_TOP + row as f32 * line_h - self.scroll_y;
                let cell_in_row = cells - (row - self.wrap.rows_before(line)) * per_row;
                scene.overlays.push(Quad::new(
                    cx + cell_in_row as f32 * advance,
                    y,
                    CARET_WIDTH,
                    line_h - 2.0,
                    self.theme.caret,
                ));
            }
        }
    }

    /// Quad de selección/coincidencia recortado a las filas visibles, con
    /// envoltura: un rango que abarca varias filas pinta una por fila. Se
    /// acota primero a las líneas del viewport, así que es O(visible) y no
    /// O(rango) (importa con "seleccionar todo" en archivos grandes).
    fn push_wrapped_range(
        &self,
        scene: &mut Scene,
        range: &Range<usize>,
        color: (u8, u8, u8, u8),
        viewport_h: f32,
    ) {
        if range.is_empty() {
            return;
        }
        let per_row = self.wrap.wrap_cells.max(1);
        let line_h = self.line_height();
        let inset = self.top_inset;
        let cx = self.left_inset + self.code_offset();
        let advance = self.font_size * self.advance_ratio;
        let tab_width = self.tab_width;

        let first_line = self.buffer.char_to_line(range.start);
        let last_line = self.buffer.char_to_line(range.end.saturating_sub(1));
        let (vis_first, vis_last) = self.visible_rows(viewport_h);
        // Acotado por líneas: `line_at_row` es una búsqueda binaria sobre el
        // acumulado de filas, así que un rango enorme solo paga O(visible).
        let from_line = first_line.max(self.wrap.line_at_row(vis_first));
        let to_line = last_line.min(self.wrap.line_at_row(vis_last));

        for line in from_line..=to_line {
            let line_start = self.buffer.line_to_char(line);
            let text = self.buffer.line_text(line);
            let start_cells = if line == first_line {
                movement::visual_column(&text, range.start - line_start, tab_width)
            } else {
                0
            };
            let end_cells = if line == last_line {
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

            let row0 = self.wrap.rows_before(line);
            for row_in_line in 0..self.wrap.rows_of_line(line) {
                let row = row0 + row_in_line;
                if row < vis_first || row > vis_last {
                    continue;
                }
                let cells_from = row_in_line * per_row;
                let cell_from = start_cells.max(cells_from);
                let cell_to = end_cells.min(cells_from + per_row);
                if cell_from >= cell_to {
                    continue;
                }
                let y = inset + PADDING_TOP + row as f32 * line_h - self.scroll_y;
                scene.quads.push(Quad::new(
                    cx + (cell_from - cells_from) as f32 * advance,
                    y,
                    (cell_to - cell_from) as f32 * advance,
                    line_h,
                    color,
                ));
            }
        }
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

    /// Reconstruye la caché de envoltura si algo cambió (buffer, ancho
    /// disponible, fuente o tabulador). Con el ajuste apagado la caché queda
    /// vacía y toda la geometría sigue el camino de una fila por línea.
    /// Casi no-op cuando la caché sigue vigente.
    fn ensure_wrap(&mut self) {
        if !self.word_wrap {
            if !self.wrap.prefix.is_empty() {
                self.wrap = WrapCache::default();
            }
            return;
        }
        let cells = self.wrap_cells();
        if self
            .wrap
            .is_stale(cells, self.tab_width, self.buffer.revision())
        {
            self.wrap = WrapCache::build(&self.buffer, cells, self.tab_width);
        }
    }

    /// Celdas de pantalla que caben por fila con el ancho y la fuente actuales.
    fn wrap_cells(&self) -> usize {
        let usable = self.viewport.0 - self.left_inset - self.code_offset() - WRAP_PAD;
        ((usable / self.advance_px()).floor() as usize).max(1)
    }

    /// Advance en px por celda con el ratio medido en el último frame.
    fn advance_px(&self) -> f32 {
        (self.font_size * self.advance_ratio).max(1.0)
    }

    /// Fila de pantalla (0-based) que ocupa `pos`: la línea lógica sin
    /// ajuste, la fila de envoltura con él.
    fn row_of_pos(&mut self, pos: usize) -> usize {
        self.ensure_wrap();
        if self.word_wrap {
            self.row_of_char(pos)
        } else {
            self.buffer.char_to_line(pos)
        }
    }

    /// Fila de pantalla (con envoltura) que ocupa el offset `pos`.
    fn row_of_char(&self, pos: usize) -> usize {
        let line = self.buffer.char_to_line(pos);
        let col = pos - self.buffer.line_to_char(line);
        let text = self.buffer.line_text(line);
        let cells = movement::visual_column(&text, col, self.tab_width);
        self.row_of_line_cells(line, cells)
    }

    /// Fila de pantalla para la celda `cells` dentro de `line`. El final de
    /// línea exacto (celdas == ancho de la línea) NO salta de fila: el
    /// caret queda en el borde derecho de la última fila de la línea.
    fn row_of_line_cells(&self, line: usize, cells: usize) -> usize {
        let rows = self.wrap.rows_of_line(line);
        let per_row = self.wrap.wrap_cells.max(1);
        let row_in_line = (cells / per_row).min(rows - 1);
        self.wrap.rows_before(line) + row_in_line
    }

    /// Primera y última fila de pantalla visibles con el scroll actual
    /// (solo con envoltura; recortadas al total de filas).
    fn visible_rows(&self, viewport_h: f32) -> (usize, usize) {
        let line_h = self.line_height();
        let total = self.wrap.total_rows();
        let first = (((self.scroll_y - self.top_inset - PADDING_TOP) / line_h)
            .floor()
            .max(0.0) as usize)
            .min(total - 1);
        let last = (((self.scroll_y + viewport_h - self.top_inset - PADDING_TOP) / line_h).ceil()
            as usize)
            .min(total - 1);
        (first, last)
    }

    /// Un paso vertical: de línea lógica sin ajuste y de FILA de pantalla
    /// con él (conserva la columna dentro de la fila, como Sublime).
    fn vertical_step(&mut self, delta: isize, shift: bool) {
        if self.word_wrap {
            self.ensure_wrap();
            // `move_cursors` presta `&mut self`, así que la caché viaja
            // clonada dentro del closure (un Vec de u32 por tecla).
            let wrap = self.wrap.clone();
            let tab_width = self.tab_width;
            self.move_cursors(
                |cursor, buffer| wrap_vertical_move(buffer, cursor.caret, delta, &wrap, tab_width),
                shift,
            );
        } else {
            let tab_width = self.tab_width;
            self.move_cursors(
                |cursor, buffer| movement::vertical_move(buffer, cursor.caret, delta, tab_width),
                shift,
            );
        }
    }

    /// Coordenadas lógicas → offset de char (columna visual, media celda).
    /// Con envoltura `y` decide la FILA y `x` la columna dentro de ella.
    fn caret_at(&mut self, x: f32, y: f32, advance: f32) -> usize {
        self.ensure_wrap();
        let cell = ((x - self.left_inset - self.code_offset()) / advance)
            .floor()
            .max(0.0) as usize;
        let row_f =
            ((y - self.top_inset - PADDING_TOP + self.scroll_y) / self.line_height()).floor();
        let row = row_f.max(0.0) as usize;
        if self.word_wrap {
            let row = row.min(self.wrap.total_rows() - 1);
            let line = self.wrap.line_at_row(row);
            let cells = (row - self.wrap.rows_before(line)) * self.wrap.wrap_cells.max(1) + cell;
            let text = self.buffer.line_text(line);
            let char_col = movement::char_col_at_visual(&text, cells, self.tab_width);
            self.buffer.line_to_char(line) + char_col
        } else {
            let line = row.min(self.buffer.line_count() - 1);
            let text = self.buffer.line_text(line);
            let char_col = movement::char_col_at_visual(&text, cell, self.tab_width);
            self.buffer.line_to_char(line) + char_col
        }
    }

    fn page_lines(&self) -> isize {
        ((self.viewport.1 / self.line_height()) as isize).max(1)
    }

    fn scroll_into_view(&mut self) {
        let caret = self.primary().caret;
        self.scroll_position_into_view(caret);
    }

    /// Garantiza que la posición `pos` (un offset de char) quede a la vista.
    fn scroll_position_into_view(&mut self, pos: usize) {
        let row = self.row_of_pos(pos);
        let line_h = self.line_height();
        let top = self.top_inset + PADDING_TOP + row as f32 * line_h;
        let bottom = top + line_h;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + self.viewport.1 {
            self.scroll_y = bottom - self.viewport.1;
        }
        self.clamp_scroll();
    }

    /// ¿El caret primario está a la vista? Si no lo está, el parpadeo puede
    /// saltarse el repintar (el estado de blink se conserva).
    pub fn caret_in_view(&mut self) -> bool {
        let row = self.row_of_pos(self.primary().caret);
        let line_h = self.line_height();
        let top = self.top_inset + PADDING_TOP + row as f32 * line_h;
        let bottom = top + line_h;
        bottom > self.scroll_y && top < self.scroll_y + self.viewport.1
    }

    fn clamp_scroll(&mut self) {
        self.ensure_wrap();
        let rows = if self.word_wrap {
            self.wrap.total_rows()
        } else {
            self.buffer.line_count()
        };
        let content_h =
            self.top_inset + PADDING_TOP + rows as f32 * self.line_height() + PADDING_TOP;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - self.viewport.1).max(0.0));
    }
}

/// Mueve el caret `delta` FILAS de pantalla manteniendo la columna dentro
/// de la fila (con envoltura eso es lo que el usuario "ve" como columna).
/// Al llegar al borde se queda donde está, igual que [`movement::vertical_move`].
fn wrap_vertical_move(
    buffer: &Buffer,
    caret: usize,
    delta: isize,
    wrap: &WrapCache,
    tab_width: usize,
) -> usize {
    let per_row = wrap.wrap_cells.max(1);
    let line = buffer.char_to_line(caret);
    let line_start = buffer.line_to_char(line);
    let text = buffer.line_text(line);
    let cells = movement::visual_column(&text, caret.saturating_sub(line_start), tab_width);
    let rows = wrap.rows_of_line(line);
    let row_in_line = (cells / per_row).min(rows - 1);
    let goal = cells - row_in_line * per_row;

    let current = wrap.rows_before(line) + row_in_line;
    let target = (current as isize + delta).clamp(0, wrap.total_rows() as isize - 1) as usize;
    if target == current {
        return caret;
    }
    let target_line = wrap.line_at_row(target);
    let target_cells = (target - wrap.rows_before(target_line)) * per_row + goal;
    let target_text = buffer.line_text(target_line);
    let char_col = movement::char_col_at_visual(&target_text, target_cells, tab_width);
    buffer.line_to_char(target_line) + char_col
}

/// Subcadena de `text` que ocupa las celdas visuales `[from, to)` de la
/// línea, recortada a ellas (nunca parte un glifo: si el límite cae en
/// medio, la media celda decide). Vacía si el tramo no tiene texto.
fn row_slice(text: &str, from: usize, to: usize, tab_width: usize) -> &str {
    if from >= to {
        return "";
    }
    let start = movement::byte_at_char(text, movement::char_col_at_visual(text, from, tab_width));
    let end = movement::byte_at_char(text, movement::char_col_at_visual(text, to, tab_width));
    text.get(start..end).unwrap_or("")
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
    line_height: f32,
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
        line_height,
    } = ctx;
    let first = buffer.char_to_line(range.start);
    let last = buffer.char_to_line(range.end.saturating_sub(1));
    // Solo la intersección con lo visible: O(viewport), no O(rango).
    for line in visible_line_range(first, last, visible_first, visible_last) {
        let y = inset + PADDING_TOP + line as f32 * line_height - scroll_y;
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
            scene.quads.push(Quad::new(x, y, w, line_height, color));
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

/// Posiciones (chars) de cada coincidencia de `re` en el buffer, en orden.
/// Los matches de ancho cero (`^`, `\b`…) no se listan: no se pueden ni
/// resaltar ni reemplazar. Un error de backtracking corta la lista (nunca
/// pánic: la regex compilada sigue siendo del usuario).
fn find_in_buffer_regex(buffer: &Buffer, re: &fancy_regex::Regex) -> Vec<Range<usize>> {
    let hay = buffer.text_range(0..buffer.len_chars());
    let mut found = Vec::new();
    let mut prev_byte = 0;
    let mut prev_chars = 0;
    for caps in re.captures_iter(&hay) {
        let Ok(caps) = caps else { break };
        let Some(m) = caps.get(0) else { continue };
        if m.start() == m.end() {
            continue;
        }
        // Conversión byte→char incremental (como en find_in_buffer).
        let start = prev_chars + hay[prev_byte..m.start()].chars().count();
        let len = hay[m.start()..m.end()].chars().count();
        prev_byte = m.start();
        prev_chars = start;
        found.push(start..start + len);
    }
    found
}

/// Expansión del reemplazo con los captures de `re` sobre la coincidencia
/// exacta `range` (fancy-regex `Captures::expand`: `$1`, `$name`…); si no
/// hay coincidencia exacta, texto literal.
fn expand_regex_replacement(
    re: &fancy_regex::Regex,
    text: &str,
    range: &Range<usize>,
    replacement: &str,
) -> String {
    let mut prev_byte = 0;
    let mut prev_chars = 0;
    for caps in re.captures_iter(text) {
        let Ok(caps) = caps else { break };
        let Some(m) = caps.get(0) else { continue };
        if m.start() == m.end() {
            continue;
        }
        let start = prev_chars + text[prev_byte..m.start()].chars().count();
        let len = text[m.start()..m.end()].chars().count();
        prev_byte = m.start();
        prev_chars = start;
        if (start..start + len) == *range {
            let mut expanded = String::new();
            caps.expand(replacement, &mut expanded);
            return expanded;
        }
    }
    replacement.to_owned()
}

/// Cierre que auto_match escribe tras este apertura (paréntesis, corchetes,
/// llaves y comillas; las comillas son su propio cierre).
fn auto_closer_of(ch: char) -> Option<char> {
    match ch {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

/// ¿Juntos forman un par que Backspace debe borrar de una vez?
fn is_auto_pair(prev: &str, next: &str) -> bool {
    matches!(
        (prev, next),
        ("(", ")") | ("[", "]") | ("{", "}") | ("\"", "\"") | ("'", "'")
    )
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
    fn ctrl_arrow_moves_by_words_and_ctrl_backspace_deletes() {
        let mut editor = Editor::default();
        editor.insert_text("foo bar baz");
        editor.cursors = vec![Cursor::new(0)];
        let left = Key::Named(NamedKey::ArrowLeft);
        let right = Key::Named(NamedKey::ArrowRight);
        let ctrl = ModifiersState::CONTROL;
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;

        editor.handle_key(&right, ctrl);
        assert_eq!(editor.primary().caret, 4, "inicio de bar");
        editor.handle_key(&right, ctrl);
        assert_eq!(editor.primary().caret, 8, "inicio de baz");
        editor.handle_key(&right, ctrl);
        assert_eq!(editor.primary().caret, 11, "al final del buffer");
        editor.handle_key(&left, ctrl);
        assert_eq!(editor.primary().caret, 8, "vuelve a baz");
        editor.handle_key(&left, ctrl);
        assert_eq!(editor.primary().caret, 4, "vuelve a bar");
        editor.handle_key(&left, ctrl);
        assert_eq!(editor.primary().caret, 0, "vuelve a foo");

        // Con Shift la palabra queda seleccionada.
        editor.handle_key(&right, ctrl_shift);
        assert_eq!(editor.primary().range(), 0..4);

        // Ctrl+Backspace con selección la borra como Backspace normal.
        editor.handle_key(&Key::Named(NamedKey::Backspace), ctrl);
        assert_eq!(text_of(&editor), "bar baz", "borra la selección 0..4");

        // Sin selección borra hasta el inicio de la palabra anterior.
        editor.handle_key(&right, ctrl); // al inicio de "baz"
        editor.handle_key(&Key::Named(NamedKey::Backspace), ctrl);
        assert_eq!(
            text_of(&editor),
            "baz",
            "desde el inicio de baz borra 'bar ' entero"
        );
        // El historial fusiona deletes contiguos: el undo restaura el bloque.
        assert!(editor.undo());
        assert_eq!(text_of(&editor), "foo bar baz");
    }

    #[test]
    fn home_toggles_between_column_zero_and_first_non_blank() {
        let mut editor = Editor::default();
        editor.insert_text("    código");
        let home = Key::Named(NamedKey::Home);

        editor.handle_key(&home, ModifiersState::empty());
        assert_eq!(editor.primary().caret, 4, "al primer no-blanco");
        editor.handle_key(&home, ModifiersState::empty());
        assert_eq!(editor.primary().caret, 0, "de vuelta a la columna 0");
        editor.handle_key(&home, ModifiersState::empty());
        assert_eq!(editor.primary().caret, 4, "y otra vez al no-blanco");
    }

    #[test]
    fn expand_to_line_extends_one_line_per_press() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres"); // len 12
        editor.cursors = vec![Cursor::new(1)];
        editor.expand_to_line();
        assert_eq!(editor.primary().range(), 0..4, "línea 0 con su \\n");
        editor.expand_to_line();
        assert_eq!(editor.primary().range(), 0..8, "extiende a la línea 1");
        editor.expand_to_line();
        assert_eq!(editor.primary().range(), 0..12, "y a la última (sin \\n)");
        editor.expand_to_line();
        assert_eq!(editor.primary().range(), 0..12, "en el final no crece");
    }

    #[test]
    fn join_lines_inserts_one_space_and_skips_indent() {
        let mut editor = Editor::default();
        editor.insert_text("hola\n    mundo\nfin");
        editor.cursors = vec![Cursor::new(1)];
        editor.join_lines();
        assert_eq!(
            text_of(&editor),
            "hola mundo\nfin",
            "quita \\n e indent; un espacio"
        );
        editor.join_lines();
        assert_eq!(text_of(&editor), "hola mundo fin");
        editor.join_lines();
        assert_eq!(text_of(&editor), "hola mundo fin", "última línea: no-op");
        assert!(editor.undo(), "cada unión es un paso de undo");
        assert_eq!(text_of(&editor), "hola mundo\nfin");
    }

    #[test]
    fn move_line_up_and_down_swap_keeping_column() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres");
        editor.cursors = vec![Cursor::new(5)]; // 'o' de "dos" (columna 1)

        editor.move_line_up();
        assert_eq!(text_of(&editor), "dos\nuno\ntres");
        assert_eq!(
            editor.primary().caret,
            1,
            "conserva la columna en la línea movida"
        );

        editor.move_line_down();
        assert_eq!(
            text_of(&editor),
            "uno\ndos\ntres",
            "vuelve a la posición original"
        );
        assert_eq!(editor.primary().caret, 5);

        editor.cursors = vec![Cursor::new(0)];
        editor.move_line_up();
        assert_eq!(
            text_of(&editor),
            "uno\ndos\ntres",
            "no-op en la primera línea"
        );

        editor.cursors = vec![Cursor::new(12)];
        editor.move_line_down();
        assert_eq!(text_of(&editor), "uno\ndos\ntres", "no-op en la última");
    }

    #[test]
    fn tab_key_indents_selection_and_shift_tab_unindents() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos");
        editor.cursors = vec![Cursor {
            caret: 2,
            anchor: 6,
        }];

        editor.handle_key(&Key::Named(NamedKey::Tab), ModifiersState::empty());
        assert_eq!(text_of(&editor), "\tuno\n\tdos");
        assert_eq!(editor.primary().caret, 3, "caret desplazado por el tab");

        editor.handle_key(&Key::Named(NamedKey::Tab), ModifiersState::SHIFT);
        assert_eq!(text_of(&editor), "uno\ndos");
        assert_eq!(editor.primary().caret, 2);
    }

    #[test]
    fn translate_tabs_to_spaces_and_unindent_removes_one_level() {
        let mut editor = Editor::default();
        editor.set_tab_width(2);
        editor.set_translate_tabs_to_spaces(true);
        editor.insert_text("a\n  b");
        editor.cursors = vec![Cursor {
            caret: 0,
            anchor: 6,
        }];
        editor.indent_selection();
        assert_eq!(text_of(&editor), "  a\n    b", "añade 2 espacios por línea");
        editor.unindent_selection();
        assert_eq!(text_of(&editor), "a\n  b", "quita un nivel de espacios");
        editor.unindent_selection();
        assert_eq!(text_of(&editor), "a\nb", "y el segundo nivel");

        // Con tabulador, Shift+Tab quita un solo \t aunque tab_width > 1.
        let mut editor = Editor::default();
        editor.set_tab_width(4);
        editor.insert_text("\t\tx");
        editor.cursors = vec![Cursor::new(3)];
        editor.unindent_selection();
        assert_eq!(text_of(&editor), "\tx");
    }

    #[test]
    fn enter_carries_the_indentation() {
        let mut editor = Editor::default();
        editor.insert_text("    código");
        editor.cursors = vec![Cursor::new(10)];
        editor.handle_key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
        assert_eq!(text_of(&editor), "    código\n    ");
        assert_eq!(editor.primary().caret, 15);

        // Con selección Enter la borra e inserta solo el \n (como antes).
        let mut editor = Editor::default();
        editor.insert_text("  ab");
        editor.cursors = vec![Cursor {
            caret: 2,
            anchor: 4,
        }];
        editor.handle_key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
        assert_eq!(text_of(&editor), "  \n");
    }

    #[test]
    fn goto_line_moves_caret_and_clamps() {
        let mut editor = Editor::default();
        editor.insert_text("uno\ndos\ntres");
        editor.goto_line(2);
        assert_eq!(editor.primary().caret, 4);
        editor.goto_line(99);
        assert_eq!(editor.primary().caret, 8, "clamp a la última línea");
        editor.goto_line(1);
        assert_eq!(editor.primary().caret, 0, "línea 1 = la primera");
    }

    #[test]
    fn status_text_reports_ln_col_selection_tab_and_lang() {
        let mut editor = Editor::default();
        editor.insert_text("hola\nmundo");
        editor.cursors = vec![Cursor::new(8)]; // antes de la 'd' de "mundo"
        assert_eq!(
            editor.status_text(),
            "Ln 2, Col 4  |  Tab: 4  |  Texto plano  |  sin título"
        );

        editor.cursors = vec![Cursor {
            caret: 8,
            anchor: 4,
        }];
        assert!(
            editor.status_text().contains("2 líneas, 4 chars"),
            "{}",
            editor.status_text()
        );

        editor.set_translate_tabs_to_spaces(true);
        assert!(editor.status_text().contains("Esp: 4"), "tab → espacios");
    }

    #[test]
    fn auto_match_wraps_pairs_and_skips_closers() {
        let mut editor = Editor::default();
        let open = Key::Character("(".into());
        let close = Key::Character(")".into());

        editor.handle_key(&open, ModifiersState::empty());
        assert_eq!(text_of(&editor), "()", "escribe el par entero");
        assert_eq!(editor.primary().caret, 1, "el caret queda en medio");

        editor.handle_key(&close, ModifiersState::empty());
        assert_eq!(text_of(&editor), "()", "skip-over: no duplica el cierre");
        assert_eq!(editor.primary().caret, 2);

        editor.insert_text("x");
        editor.handle_key(&Key::Character("\"".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "()x\"\"", "comillas también");
        // Cada par deja el caret dentro: se salta al final para el siguiente.
        editor.cursors = vec![Cursor::new(editor.buffer.len_chars())];
        editor.handle_key(&Key::Character("'".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "()x\"\"''", "y comilla simple");
        editor.cursors = vec![Cursor::new(editor.buffer.len_chars())];
        editor.handle_key(&Key::Character("{".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "()x\"\"''{}", "llaves");
        editor.cursors = vec![Cursor::new(editor.buffer.len_chars())];
        editor.handle_key(&Key::Character("[".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "()x\"\"''{}[]", "corchetes");
    }

    #[test]
    fn backspace_between_auto_pair_deletes_both() {
        let mut editor = Editor::default();
        editor.handle_key(&Key::Character("(".into()), ModifiersState::empty());
        editor.backspace();
        assert_eq!(text_of(&editor), "", "borra () de una vez");
        assert!(editor.undo(), "es una sola transacción");
        assert_eq!(text_of(&editor), "()");

        // Entre un par escrito a mano también, pero con selección no.
        let mut editor = Editor::default();
        editor.insert_text("a()c");
        editor.cursors = vec![Cursor::new(2)];
        editor.backspace();
        assert_eq!(text_of(&editor), "ac", "par manual: borra los dos");
        editor.cursors = vec![Cursor {
            caret: 0,
            anchor: 2,
        }];
        editor.backspace();
        assert_eq!(text_of(&editor), "", "con selección solo la selección");
    }

    #[test]
    fn auto_match_off_selection_and_multicursor_write_plain() {
        let mut editor = Editor::default();
        editor.set_auto_match(false);
        editor.handle_key(&Key::Character("(".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "(", "auto_match desactivado");

        let mut editor = Editor::default();
        editor.insert_text("ab");
        editor.cursors = vec![Cursor {
            caret: 1,
            anchor: 2,
        }];
        editor.handle_key(&Key::Character("(".into()), ModifiersState::empty());
        assert_eq!(
            text_of(&editor),
            "a(",
            "con selección reemplaza sin emparejar"
        );

        let mut editor = Editor::default();
        editor.insert_text("ab cd"); // caret 3 = antes de la 'c', tras el espacio
        editor.cursors = vec![Cursor::new(0), Cursor::new(3)];
        editor.handle_key(&Key::Character("(".into()), ModifiersState::empty());
        assert_eq!(text_of(&editor), "(ab (cd", "multi-cursor: inserta plano");
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
    fn find_regex_matches_and_keeps_smart_case() {
        let mut editor = Editor::default();
        editor.insert_text("Foo bar foo FOO");
        editor.cursors = vec![Cursor::new(0)];
        editor.set_find_regex(true);

        editor.find_set_query("fo+");
        assert_eq!(
            editor.find_stats(),
            (1, 3),
            "regex sin mayúsculas: insensible como el literal"
        );

        editor.find_set_query("Fo+");
        assert_eq!(
            editor.find_stats(),
            (1, 1),
            "con mayúscula: smart-case también en regex"
        );

        editor.set_find_regex(false);
        assert_eq!(
            editor.find_stats(),
            (0, 0),
            "volver a literal reinterpreta la consulta: 'Fo+' no está en el texto"
        );
        assert!(!editor.find_regex_invalid());
    }

    #[test]
    fn find_regex_invalid_reports_and_zero_width_matches_are_ignored() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo");
        editor.cursors = vec![Cursor::new(0)];
        editor.set_find_regex(true);

        editor.find_set_query("(");
        assert_eq!(
            editor.find_stats(),
            (0, 0),
            "patrón inválido: sin coincidencias"
        );
        assert!(editor.find_regex_invalid(), "y la barra puede avisar");

        editor.find_set_query("mundo");
        assert!(!editor.find_regex_invalid(), "patrón válido repara el flag");
        assert_eq!(editor.find_stats(), (1, 1));

        editor.find_set_query("^");
        assert_eq!(
            editor.find_stats(),
            (0, 0),
            "zero-width (^): no se lista ni se reemplaza"
        );
        assert!(!editor.find_regex_invalid(), "'^' sí compila");
    }

    #[test]
    fn find_regex_replace_expands_captures() {
        let mut editor = Editor::default();
        editor.insert_text("a@x b@y");
        editor.cursors = vec![Cursor::new(0)];
        editor.set_find_regex(true);
        editor.find_set_query(r"(\w+)@(\w+)");

        assert_eq!(
            editor.find_replace_all("$2:$1"),
            2,
            "cuenta las dos coincidencias"
        );
        assert_eq!(text_of(&editor), "x:a y:b", "$1/$2 se expanden por capture");
        assert!(editor.undo(), "un solo undo para todo");
        assert_eq!(text_of(&editor), "a@x b@y");

        // Reemplazo puntual también expande.
        editor.find_set_query(r"(\w+)@(\w+)");
        assert!(editor.find_replace_current("<$2>"));
        assert_eq!(text_of(&editor), "<x> b@y");
    }

    #[test]
    fn from_hot_restores_text_dirty_and_path() {
        let editor = Editor::from_hot(
            Some(std::path::PathBuf::from("nota.md")),
            "contenido sin guardar",
        );
        assert_eq!(text_of(&editor), "contenido sin guardar");
        assert!(
            editor.dirty(),
            "hot exit arranca sucio (el disco puede estar viejo)"
        );
        assert_eq!(
            editor.path().map(|p| p.display().to_string()),
            Some("nota.md".to_owned())
        );
        assert_eq!(editor.tab_label(), "nota.md");

        let untitled = Editor::from_hot(None, "borrador");
        assert_eq!(untitled.tab_label(), "sin título");
        assert!(untitled.dirty());
        assert!(!untitled.has_path());
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
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
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
    fn font_size_scales_line_height_advance_and_gutter() {
        let mut editor = Editor::default();
        assert_eq!(editor.line_height(), LINE_HEIGHT, "14 px → 20 px exactos");
        assert_eq!(editor.code_offset(), CODE_X, "gutter por defecto");
        assert_eq!(editor.gutter_right(), GUTTER_RIGHT_X);

        editor.set_font_size(28);
        assert_eq!(editor.line_height(), 40.0, "misma proporción 10/7");
        assert_eq!(editor.code_offset(), CODE_X * 2.0, "el código se desplaza");
        assert_eq!(editor.gutter_right(), GUTTER_RIGHT_X * 2.0);

        editor.set_font_size(1);
        assert_eq!(editor.font_size, 8.0, "suelo de 8 px");
        editor.set_font_size(999);
        assert_eq!(editor.font_size, 72.0, "techo de 72 px");
    }

    #[test]
    fn build_scene_uses_the_configured_font_size() {
        let mut editor = Editor::default();
        editor.insert_text("hola\nmundo");

        let small = editor.build_scene((600.0, 200.0), 1.0);
        let small_line2 = small
            .texts
            .iter()
            .find(|span| span.text == "mundo")
            .expect("la segunda línea está a la vista");
        assert_eq!(small_line2.y, PADDING_TOP + LINE_HEIGHT);
        assert_eq!(small_line2.size, FONT_SIZE);

        editor.set_font_size(28);
        let big = editor.build_scene((600.0, 200.0), 1.0);
        let big_line2 = big
            .texts
            .iter()
            .find(|span| span.text == "mundo")
            .expect("la segunda línea sigue a la vista");
        assert_eq!(big_line2.y, PADDING_TOP + 40.0, "línea a 40 px");
        assert_eq!(big_line2.size, 28.0, "texto en el tamaño pedido");
        assert_eq!(big.overlays[0].h, 40.0 - 2.0, "caret = línea − 2 px");
        let number = big
            .texts
            .iter()
            .find(|span| span.text == "1")
            .expect("número de línea");
        assert_eq!(
            number.x,
            GUTTER_RIGHT_X * 2.0 - 28.0,
            "una cifra alineada a la derecha del gutter escalado"
        );
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

    //── Ajuste de línea (word wrap, M24a) ──────────────────────────────────
    //
    // Con viewport 640 px, gutter de 64 y holgura de 8, a 14 px de advance
    // caben (640 − 64 − 8) / 14 = 40 celdas por fila de pantalla.

    /// Editor con ajuste activo, ratio de fuente ya medido y scroll a cero.
    fn wrapped(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.insert_text(text);
        editor.set_viewport(640.0, 200.0);
        editor.set_word_wrap(true);
        editor.build_scene((640.0, 200.0), 1.0);
        editor.scroll_y = 0.0;
        editor
    }

    #[test]
    fn word_wrap_counts_screen_rows_and_slices_each_one() {
        let mut editor = wrapped(&format!("{}\nfin\n", "a".repeat(100)));
        editor.cursors = vec![Cursor::new(0)];
        let scene = editor.build_scene((640.0, 200.0), 1.0);

        assert_eq!(editor.wrap.wrap_cells, 40, "40 celdas por fila");
        assert_eq!(editor.wrap.total_rows(), 5, "3 filas + «fin» + la vacía");
        assert_eq!(editor.wrap.rows_of_line(0), 3, "100 chars en 40 celdas");
        assert_eq!(editor.wrap.rows_before(1), 3, "«fin» empieza en la fila 3");
        assert_eq!(editor.wrap.line_at_row(2), 0);
        assert_eq!(editor.wrap.line_at_row(3), 1);
        assert_eq!(editor.wrap.line_at_row(4), 2);

        // Las 3 filas de la línea larga se pintan en x = código, sin
        // solaparse y reconstruyendo el texto entero.
        let pieces: Vec<&TextSpan> = scene
            .texts
            .iter()
            .filter(|span| span.x == CODE_X && span.y < 60.0)
            .collect();
        assert_eq!(pieces.len(), 3, "una fila de texto cada 40 celdas");
        assert_eq!(pieces[0].y, PADDING_TOP);
        assert_eq!(pieces[1].y, PADDING_TOP + LINE_HEIGHT);
        assert_eq!(pieces[2].y, PADDING_TOP + 2.0 * LINE_HEIGHT);
        let joined: String = pieces.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(joined, "a".repeat(100), "ni se pierde ni se repite nada");
        // El número de línea solo aparece en la PRIMERA fila de cada línea.
        assert!(
            scene
                .texts
                .iter()
                .any(|span| span.text == "1" && span.y == PADDING_TOP),
            "gutter en la primera fila"
        );
        assert!(
            !scene
                .texts
                .iter()
                .any(|span| span.text == "1" && span.y > PADDING_TOP),
            "sin gutter repetido en las filas de continuación"
        );
    }

    #[test]
    fn caret_at_lands_on_the_wrapped_row() {
        let mut editor = wrapped(&"a".repeat(100));
        editor.cursors = vec![Cursor::new(0)];
        let advance = 14.0;
        let row = |n: usize| PADDING_TOP + n as f32 * LINE_HEIGHT;

        assert_eq!(
            editor.caret_at(CODE_X, row(1), advance),
            40,
            "fila 1 → char 40"
        );
        assert_eq!(
            editor.caret_at(CODE_X + 5.0 * advance, row(1), advance),
            45,
            "columna 5 de la fila 1"
        );
        assert_eq!(
            editor.caret_at(CODE_X + 19.0 * advance, row(2), advance),
            99
        );
        assert_eq!(
            editor.caret_at(CODE_X, row(3), advance),
            80,
            "la fila 3 no existe: se recorta a la última"
        );
    }

    #[test]
    fn arrow_steps_walk_screen_rows_with_wrap() {
        let mut editor = wrapped(&format!("{}\n{}", "a".repeat(50), "b".repeat(60)));
        editor.cursors = vec![Cursor::new(0)];

        assert_eq!(editor.wrap.total_rows(), 4, "50 → 2 filas y 60 → 2 filas");

        editor.vertical_step(1, false);
        assert_eq!(
            editor.primary().caret,
            40,
            "fila 1 = misma línea, +40 celdas"
        );
        editor.vertical_step(1, false);
        assert_eq!(editor.primary().caret, 51, "fila 2 = inicio de la 2ª línea");
        editor.vertical_step(-1, false);
        assert_eq!(editor.primary().caret, 40, "y vuelta a la fila 1");
    }

    #[test]
    fn selection_quads_split_into_one_per_row() {
        let mut editor = wrapped(&"a".repeat(100));
        editor.cursors = vec![Cursor {
            caret: 100,
            anchor: 0,
        }];

        let scene = editor.build_scene((640.0, 200.0), 1.0);
        assert_eq!(scene.quads.len(), 3, "un quad por cada fila de pantalla");
        assert_eq!(scene.quads[0].x, CODE_X);
        assert_eq!(scene.quads[0].w, 40.0 * 14.0, "primera fila llena");
        assert_eq!(scene.quads[1].y, PADDING_TOP + LINE_HEIGHT);
        assert_eq!(scene.quads[2].w, 20.0 * 14.0, "la última solo 20 celdas");
        assert_eq!(
            scene.overlays.len(),
            1,
            "el caret va en overlays, no cuenta como quad"
        );
    }

    #[test]
    fn wrapped_syntax_tiles_every_row_without_losing_text() {
        let long = "fn an_example_function(argument: u32) -> u32 { argument + 1 }";
        assert!(long.chars().count() > 40, "la línea desborda una fila");
        let mut editor = Editor {
            lang: "rs".to_owned(),
            ..Editor::default()
        };
        editor.insert_text(long);
        editor.set_viewport(640.0, 200.0);
        editor.set_word_wrap(true);
        editor.build_scene((640.0, 200.0), 1.0);
        editor.scroll_y = 0.0;
        editor.cursors = vec![Cursor::new(0)];

        let scene = editor.build_scene((640.0, 200.0), 1.0);
        assert_eq!(editor.wrap.wrap_cells, 40);
        assert_eq!(editor.wrap.total_rows(), 2);

        let rows = [PADDING_TOP, PADDING_TOP + LINE_HEIGHT];
        let mut rebuilt = String::new();
        for y in rows {
            let spans: Vec<&TextSpan> = scene
                .texts
                .iter()
                .filter(|span| span.x >= CODE_X && span.y == y)
                .collect();
            assert!(!spans.is_empty(), "la fila a y={y} pinta algo del código");
            for span in spans {
                assert!(
                    span.x < CODE_X + 40.0 * 14.0,
                    "ninguna pieza empieza fuera de la fila"
                );
                rebuilt.push_str(&span.text);
            }
        }
        assert_eq!(
            rebuilt, long,
            "los trozos recortados en la frontera cuadran"
        );
    }

    #[test]
    fn wrap_scroll_and_caret_view_count_rows_not_lines() {
        let mut editor = wrapped(&"a".repeat(100));
        editor.set_viewport(640.0, 60.0);
        editor.build_scene((640.0, 60.0), 1.0);
        editor.cursors = vec![Cursor::new(45)];

        // 100 chars en 3 filas → 8 + 3*20 + 8 = 76 px de contenido.
        editor.scroll_y = 1_000.0;
        editor.clamp_scroll();
        assert_eq!(
            editor.scroll_y, 16.0,
            "el tope cuenta FILAS (3), no líneas (1)"
        );

        editor.scroll_y = 0.0;
        assert!(editor.caret_in_view(), "la fila 1 (y=28) cabe en 60 px");
        editor.scroll_y = 100.0;
        assert!(!editor.caret_in_view(), "por encima de la fila del caret");

        // Con el ajuste apagado el mismo buffer es UNA línea: no hay nada
        // que desplazar y el caret (fila lógica 0) vuelve a caber.
        editor.set_word_wrap(false);
        assert_eq!(editor.scroll_y, 0.0, "una sola línea cabe en 60 px");
    }
}
