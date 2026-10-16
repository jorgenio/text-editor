//! Barra de buscar/reemplazar (Ctrl+F / Ctrl+H) al pie de la ventana.
//!
//! El estado de las coincidencias vive en el [`Editor`] (smart-case,
//! resaltado, navegación y reemplazos): esta barra solo edita las dos
//! consultas y decide a dónde va cada tecla. Se dibuja en la capa superior,
//! por debajo de la paleta de comandos. La consulta se conserva al cerrar.

use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::editor::{Editor, FONT_SIZE};

/// Alto de cada fila (px lógicos).
const ROW_H: f32 = 30.0;
/// Margen interior de la barra (px lógicos).
const PAD: f32 = 8.0;
/// Franja de borde superior de la barra.
const EDGE: f32 = 1.0;

const PANEL_BG: (u8, u8, u8, u8) = (0x1E, 0x1F, 0x1C, 0xFF);
const PANEL_EDGE: (u8, u8, u8, u8) = (0x3E, 0x3D, 0x32, 0xFF);
const FIELD_BG: (u8, u8, u8, u8) = (0x14, 0x15, 0x12, 0xFF);
const BUTTON_BG: (u8, u8, u8, u8) = (0x49, 0x48, 0x3E, 0xFF);
const FG: (u8, u8, u8) = (0xF8, 0xF8, 0xF2);
const DIM: (u8, u8, u8) = (0x75, 0x71, 0x5E);
/// Caret de los campos (mismo blanco que FG, pero con alfa para los quads).
const CARET: (u8, u8, u8, u8) = (0xF8, 0xF8, 0xF2, 0xFF);

/// Qué campo edita la barra (y a dónde van las teclas de texto).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Query,
    Replacement,
}

/// Rectángulo de la barra para el hit-test del ratón.
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Layout del último render, guardado para [`SearchBar::handle_click`].
#[derive(Clone, Copy)]
struct Layout {
    bar: Rect,
    find_field: Rect,
    /// `w == 0` cuando no está la fila de reemplazo.
    replace_field: Rect,
    btn_replace: Rect,
    btn_all: Rect,
    advance: f32,
}

pub struct SearchBar {
    open: bool,
    /// Fila de reemplazo visible (Ctrl+H la muestra).
    replace_mode: bool,
    query: String,
    replacement: String,
    field: Field,
    /// Caret del campo activo, en CHARS.
    caret: usize,
    last: Option<Layout>,
}

impl Default for SearchBar {
    fn default() -> Self {
        Self {
            open: false,
            replace_mode: false,
            query: String::new(),
            replacement: String::new(),
            field: Field::Query,
            caret: 0,
            last: None,
        }
    }
}

impl SearchBar {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Abre la barra (`replace` añade la fila de reemplazo) y sincroniza la
    /// búsqueda del editor. Si hay selección de UNA línea, es la consulta
    /// inicial; si no, se conserva la anterior.
    pub fn open(&mut self, replace: bool, prefill: Option<&str>, editor: &mut Editor) {
        self.open = true;
        self.replace_mode |= replace;
        if let Some(text) = prefill {
            self.query = text.to_owned();
            self.field = Field::Query;
        }
        self.caret = self.active_len();
        editor.find_set_query(&self.query);
    }

    /// Cierra la barra: la consulta se guarda, el modo no (Esc = reset).
    pub fn close(&mut self) {
        self.open = false;
        self.replace_mode = false;
    }

    /// Texto del IME dentro de la barra abierta.
    pub fn push_text(&mut self, text: &str, editor: &mut Editor) {
        if !self.open || text.is_empty() {
            return;
        }
        self.insert_active(text);
        self.sync_query(editor);
    }

    /// Teclado de la barra (solo mientras está abierta). `main` la llama
    /// DESPUÉS del keymap y de la paleta: aquí se traga todo lo demás.
    pub fn on_key(&mut self, event: &KeyEvent, modifiers: ModifiersState, editor: &mut Editor) {
        if event.state == ElementState::Released || !self.open {
            return;
        }
        self.handle_key(&event.logical_key, modifiers, editor);
    }

    /// Núcleo de [`SearchBar::on_key`] (separable para tests: `KeyEvent` no
    /// es construible fuera de winit).
    fn handle_key(&mut self, key: &Key, modifiers: ModifiersState, editor: &mut Editor) {
        if !self.open {
            return;
        }
        let ctrl = modifiers.control_key() || modifiers.super_key();
        let alt = modifiers.alt_key();
        let shift = modifiers.shift_key();
        match key {
            Key::Named(NamedKey::Escape) => {
                self.close();
                editor.find_close();
            }
            Key::Named(NamedKey::Enter) if ctrl && shift => {
                editor.find_replace_all(&self.replacement);
            }
            Key::Named(NamedKey::Enter) if ctrl => {
                editor.find_replace_current(&self.replacement);
            }
            Key::Named(NamedKey::Enter) => editor.find_next(shift),
            Key::Named(NamedKey::F3) => editor.find_next(shift),
            Key::Named(NamedKey::ArrowUp) => editor.find_next(true),
            Key::Named(NamedKey::ArrowDown) => editor.find_next(false),
            Key::Named(NamedKey::Tab) if !ctrl && !alt => {
                let next = match self.field {
                    Field::Query => Field::Replacement,
                    Field::Replacement => Field::Query,
                };
                if next == Field::Replacement && !self.replace_mode {
                    return;
                }
                self.field = next;
                self.caret = self.active_len();
            }
            Key::Named(NamedKey::ArrowLeft) if !ctrl && !alt => {
                self.caret = self.caret.saturating_sub(1);
            }
            Key::Named(NamedKey::ArrowRight) if !ctrl && !alt => {
                self.caret = (self.caret + 1).min(self.active_len());
            }
            Key::Named(NamedKey::Home) if !ctrl && !alt => self.caret = 0,
            Key::Named(NamedKey::End) if !ctrl && !alt => self.caret = self.active_len(),
            Key::Named(NamedKey::Backspace) if !ctrl && !alt => self.backspace(editor),
            Key::Named(NamedKey::Delete) if !ctrl && !alt => self.delete_forward(editor),
            // Imprimibles: salvo Ctrl/Cmd puro; AltGr (Ctrl+Alt) sí escribe.
            Key::Character(text) if !ctrl || alt => {
                self.insert_active(text);
                self.sync_query(editor);
            }
            _ => {}
        }
    }

    /// Ratón: `true` = consumido (el click no llega al editor). Los botones
    /// reemplazan ya; los campos fijan el caret; lo demás dentro de la barra
    /// se traga (la búsqueda NO se cierra al pinchar fuera: Sublime tampoco).
    pub fn handle_click(&mut self, x: f32, y: f32, editor: &mut Editor) -> bool {
        if !self.open {
            return false;
        }
        let Some(layout) = self.last else {
            return true; // aún sin layout: se traga el primer click
        };
        if !layout.bar.contains(x, y) {
            return false;
        }
        if layout.btn_all.contains(x, y) {
            editor.find_replace_all(&self.replacement);
            return true;
        }
        if layout.btn_replace.contains(x, y) {
            editor.find_replace_current(&self.replacement);
            return true;
        }
        if layout.find_field.contains(x, y) {
            self.field = Field::Query;
            self.caret = chars_at(&self.query, layout.find_field, x, layout.advance);
            return true;
        }
        if self.replace_mode && layout.replace_field.contains(x, y) {
            self.field = Field::Replacement;
            self.caret = chars_at(&self.replacement, layout.replace_field, x, layout.advance);
            return true;
        }
        true
    }

    /// Dibuja la barra en la capa superior (si está abierta) y guarda el
    /// layout para [`SearchBar::handle_click`].
    pub fn render(
        &mut self,
        scene: &mut Scene,
        viewport: (f32, f32),
        advance: f32,
        editor: &Editor,
    ) {
        if !self.open {
            self.last = None;
            return;
        }
        let rows = if self.replace_mode { 2 } else { 1 };
        let bar_h = EDGE + rows as f32 * ROW_H;
        let bar_y = (viewport.1 - bar_h).max(0.0);
        let bar = Rect::new(0.0, bar_y, viewport.0, bar_h);
        scene
            .top_quads
            .push(Quad::new(bar.x, bar.y, bar.w, bar.h, PANEL_BG));
        scene
            .top_quads
            .push(Quad::new(0.0, bar_y, viewport.0, EDGE, PANEL_EDGE));

        // Fila de buscar: etiqueta + campo + contador "actual / total".
        let find_row_y = bar_y + EDGE;
        let label = "Buscar:";
        let field_x = PAD + label.chars().count() as f32 * advance + 6.0;
        let (current, total) = editor.find_stats();
        let counter = if self.query.is_empty() {
            String::new()
        } else if total == 0 {
            "0 / 0".to_owned()
        } else {
            format!("{current} / {total}")
        };
        let counter_w = counter.chars().count() as f32 * advance;
        let field_w = (viewport.0 - field_x - PAD - counter_w - 8.0).max(60.0);
        let find_field = Rect::new(field_x, find_row_y + 4.0, field_w, ROW_H - 8.0);
        let text_y = find_row_y + (ROW_H - FONT_SIZE) / 2.0;
        scene
            .top_texts
            .push(TextSpan::new(label, PAD, text_y, FONT_SIZE, FG));
        let caret = (self.field == Field::Query).then_some(self.caret);
        push_field(scene, find_field, &self.query, caret, advance);
        if !counter.is_empty() {
            scene.top_texts.push(TextSpan::new(
                counter,
                viewport.0 - PAD - counter_w,
                text_y,
                FONT_SIZE,
                DIM,
            ));
        }

        // Fila de reemplazar (solo en modo reemplazo): etiqueta, campo y
        // los dos botones, alineados a la derecha.
        let mut replace_field = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut btn_replace = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut btn_all = Rect::new(0.0, 0.0, 0.0, 0.0);
        if self.replace_mode {
            let row_y = find_row_y + ROW_H;
            let label = "Reemplazar:";
            let field_x = PAD + label.chars().count() as f32 * advance + 6.0;
            let all_w = "Reemplazar todo".chars().count() as f32 * advance + 16.0;
            let one_w = "Reemplazar".chars().count() as f32 * advance + 16.0;
            let all_x = viewport.0 - PAD - all_w;
            let one_x = all_x - 8.0 - one_w;
            replace_field = Rect::new(
                field_x,
                row_y + 4.0,
                (one_x - 8.0 - field_x).max(60.0),
                ROW_H - 8.0,
            );
            btn_replace = Rect::new(one_x, row_y + 4.0, one_w, ROW_H - 8.0);
            btn_all = Rect::new(all_x, row_y + 4.0, all_w, ROW_H - 8.0);

            scene.top_texts.push(TextSpan::new(
                label,
                PAD,
                row_y + (ROW_H - FONT_SIZE) / 2.0,
                FONT_SIZE,
                FG,
            ));
            let caret = (self.field == Field::Replacement).then_some(self.caret);
            push_field(scene, replace_field, &self.replacement, caret, advance);
            push_button(scene, btn_replace, "Reemplazar", advance);
            push_button(scene, btn_all, "Reemplazar todo", advance);
        }

        self.last = Some(Layout {
            bar,
            find_field,
            replace_field,
            btn_replace,
            btn_all,
            advance,
        });
    }

    fn active(&self) -> &str {
        match self.field {
            Field::Query => &self.query,
            Field::Replacement => &self.replacement,
        }
    }

    fn active_mut(&mut self) -> &mut String {
        match self.field {
            Field::Query => &mut self.query,
            Field::Replacement => &mut self.replacement,
        }
    }

    fn active_len(&self) -> usize {
        self.active().chars().count()
    }

    /// Solo la consulta afecta a la búsqueda del editor.
    fn sync_query(&mut self, editor: &mut Editor) {
        if self.field == Field::Query {
            editor.find_set_query(&self.query);
        }
    }

    /// Índice byte del caret dentro del campo activo (chars → bytes).
    fn caret_byte(&self) -> usize {
        self.active()
            .char_indices()
            .nth(self.caret)
            .map_or_else(|| self.active().len(), |(byte, _)| byte)
    }

    fn insert_active(&mut self, text: &str) {
        let at = self.caret_byte();
        self.active_mut().insert_str(at, text);
        self.caret += text.chars().count();
    }

    fn backspace(&mut self, editor: &mut Editor) {
        if self.caret == 0 {
            return;
        }
        let at = self.caret_byte();
        let active = self.active_mut();
        let prev = active[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(b, _)| b);
        active.replace_range(prev..at, "");
        self.caret -= 1;
        self.sync_query(editor);
    }

    fn delete_forward(&mut self, editor: &mut Editor) {
        let at = self.caret_byte();
        let active = self.active_mut();
        if at >= active.len() {
            return;
        }
        let width = active[at..].chars().next().map_or(0, char::len_utf8);
        active.replace_range(at..at + width, "");
        self.sync_query(editor);
    }
}

/// Campo de texto con su caret (la consulta larga se muestra por la cola;
/// el caret solo se dibuja si el campo está activo).
fn push_field(scene: &mut Scene, rect: Rect, text: &str, caret: Option<usize>, advance: f32) {
    scene
        .top_quads
        .push(Quad::new(rect.x, rect.y, rect.w, rect.h, FIELD_BG));
    let y = rect.y + (rect.h - FONT_SIZE) / 2.0;
    let capacity = ((rect.w - 12.0) / advance).floor().max(1.0) as usize;
    let total = text.chars().count();
    let tail = total.saturating_sub(capacity);
    let shown: String = text.chars().skip(tail).collect();
    scene
        .top_texts
        .push(TextSpan::new(shown, rect.x + 6.0, y, FONT_SIZE, FG));
    if let Some(caret) = caret {
        let caret_in = caret.saturating_sub(tail);
        scene.top_quads.push(Quad::new(
            rect.x + 6.0 + caret_in as f32 * advance,
            rect.y + 3.0,
            2.0,
            rect.h - 6.0,
            CARET,
        ));
    }
}

/// Botón con la etiqueta centrada.
fn push_button(scene: &mut Scene, rect: Rect, label: &str, advance: f32) {
    scene
        .top_quads
        .push(Quad::new(rect.x, rect.y, rect.w, rect.h, BUTTON_BG));
    let x = rect.x + (rect.w - label.chars().count() as f32 * advance) / 2.0;
    let y = rect.y + (rect.h - FONT_SIZE) / 2.0;
    scene
        .top_texts
        .push(TextSpan::new(label, x, y, FONT_SIZE, FG));
}

/// Posición (chars) más cercana para un x dentro del campo `rect`, teniendo
/// en cuenta que el texto visible puede estar truncado por la cola.
fn chars_at(text: &str, rect: Rect, x: f32, advance: f32) -> usize {
    if advance <= 0.0 {
        return 0;
    }
    let capacity = ((rect.w - 12.0) / advance).floor().max(1.0) as usize;
    let total = text.chars().count();
    let tail = total.saturating_sub(capacity);
    let cell = ((x - rect.x - 6.0) / advance).round().max(0.0) as usize;
    (tail + cell).min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> ModifiersState {
        ModifiersState::empty()
    }

    fn key(ch: char) -> Key {
        Key::Character(ch.to_string().into())
    }

    fn typed(search: &mut SearchBar, editor: &mut Editor, text: &str) {
        for ch in text.chars() {
            search.handle_key(&key(ch), mods(), editor);
        }
    }

    #[test]
    fn open_prefills_from_selection_and_syncs() {
        let mut editor = Editor::default();
        editor.insert_text("hola mundo hola");
        editor.select_all();

        let mut search = SearchBar::default();
        search.open(
            false,
            editor.single_line_selection().as_deref(),
            &mut editor,
        );
        assert!(search.is_open());
        assert_eq!(search.query, "hola mundo hola", "la selección precarga");
        assert_eq!(editor.find_stats(), (1, 1), "coincidencias sincronizadas");

        // Sin selección: se conserva la consulta anterior.
        search.handle_key(&Key::Named(NamedKey::Escape), mods(), &mut editor);
        assert!(!search.is_open());
        search.open(false, None, &mut editor);
        assert_eq!(search.query, "hola mundo hola", "la consulta se guarda");
        assert_eq!(editor.find_stats(), (1, 1), "y re-sincroniza al abrir");
    }

    #[test]
    fn typing_edits_query_and_syncs_editor() {
        let mut editor = Editor::default();
        editor.insert_text("abc abc");
        let mut search = SearchBar::default();
        search.open(false, None, &mut editor);

        typed(&mut search, &mut editor, "abcd");
        assert_eq!(search.query, "abcd");
        assert_eq!(editor.find_stats(), (0, 0), "sin coincidencias");

        search.handle_key(&Key::Named(NamedKey::Backspace), mods(), &mut editor);
        assert_eq!(search.query, "abc", "backspace borra el último char");
        assert_eq!(editor.find_stats(), (1, 2), "y re-sincroniza la búsqueda");

        // El caret se mueve y edita en la posición (chars).
        search.handle_key(&Key::Named(NamedKey::Home), mods(), &mut editor);
        search.handle_key(&Key::Named(NamedKey::Delete), mods(), &mut editor);
        assert_eq!(search.query, "bc", "Delete borra hacia delante");
    }

    #[test]
    fn enter_navigates_escape_closes_and_selects() {
        let mut editor = Editor::default();
        editor.insert_text("a a");
        let mut search = SearchBar::default();
        search.open(false, None, &mut editor);
        typed(&mut search, &mut editor, "a");
        // El caret queda al final del buffer: la primera coincidencia es actual.
        assert_eq!(editor.find_stats(), (1, 2));

        search.handle_key(&Key::Named(NamedKey::Enter), mods(), &mut editor);
        assert_eq!(editor.find_stats(), (2, 2), "Enter: siguiente");
        search.handle_key(&Key::Named(NamedKey::ArrowUp), mods(), &mut editor);
        assert_eq!(editor.find_stats(), (1, 2), "Up: anterior");
        let shift = ModifiersState::SHIFT;
        search.handle_key(&Key::Named(NamedKey::Enter), shift, &mut editor);
        assert_eq!(
            editor.find_stats(),
            (2, 2),
            "Shift+Enter: anterior con vuelta"
        );
        search.handle_key(&Key::Named(NamedKey::F3), mods(), &mut editor);
        assert_eq!(editor.find_stats(), (1, 2), "F3: siguiente");

        search.handle_key(&Key::Named(NamedKey::Escape), mods(), &mut editor);
        assert!(!search.is_open(), "Escape cierra la barra");
        assert_eq!(editor.find_stats(), (0, 0), "y la búsqueda");
        assert_eq!(
            editor.single_line_selection().as_deref(),
            Some("a"),
            "la coincidencia actual queda seleccionada"
        );
    }

    #[test]
    fn tab_switches_field_and_ctrl_enter_replaces() {
        let mut editor = Editor::default();
        editor.insert_text("uno dos uno");
        let mut search = SearchBar::default();
        search.open(true, None, &mut editor);
        typed(&mut search, &mut editor, "uno");
        assert_eq!(search.field, Field::Query);
        assert_eq!(editor.find_stats(), (1, 2));

        search.handle_key(&Key::Named(NamedKey::Tab), mods(), &mut editor);
        assert_eq!(search.field, Field::Replacement, "Tab cambia de campo");
        typed(&mut search, &mut editor, "1");
        assert_eq!(search.replacement, "1");
        assert_eq!(
            editor.find_stats(),
            (1, 2),
            "escribir el reemplazo no toca la búsqueda"
        );

        let ctrl = ModifiersState::CONTROL;
        search.handle_key(&Key::Named(NamedKey::Enter), ctrl, &mut editor);
        assert_eq!(
            editor.find_stats(),
            (1, 1),
            "Ctrl+Enter reemplaza la actual"
        );

        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
        search.handle_key(&Key::Named(NamedKey::Enter), ctrl_shift, &mut editor);
        assert_eq!(editor.find_stats(), (0, 0), "Ctrl+Shift+Enter: todas");

        editor.select_all();
        assert_eq!(
            editor.single_line_selection().as_deref(),
            Some("1 dos 1"),
            "el texto quedó reemplazado"
        );
    }

    #[test]
    fn render_stores_layout_and_clicks_hit_it() {
        let mut editor = Editor::default();
        editor.insert_text("hola hola");
        let mut search = SearchBar::default();
        search.open(true, None, &mut editor);
        typed(&mut search, &mut editor, "hola");

        let mut scene = Scene::new(wgpu::Color::BLACK);
        search.render(&mut scene, (800.0, 600.0), 8.0, &editor);
        assert!(
            scene.top_quads.len() >= 4,
            "fondo + borde + campos + botones"
        );
        let layout = search.last.expect("render guarda el layout");

        // Click en el campo de buscar: activa ese campo.
        search.field = Field::Replacement;
        assert!(search.handle_click(
            layout.find_field.x + 4.0,
            layout.find_field.y + 4.0,
            &mut editor
        ));
        assert_eq!(search.field, Field::Query);

        // Click en "Reemplazar todo": reemplaza todo de una vez.
        assert!(search.handle_click(layout.btn_all.x + 4.0, layout.btn_all.y + 4.0, &mut editor));
        assert_eq!(
            editor.find_stats(),
            (0, 0),
            "sin coincidencias: todo reemplazado"
        );

        // Click fuera de la barra: no consumido (llega al editor).
        assert!(!search.handle_click(400.0, 10.0, &mut editor));
    }

    #[test]
    fn find_only_mode_hides_replace_row() {
        let mut editor = Editor::default();
        let mut search = SearchBar::default();
        search.open(false, None, &mut editor);
        assert!(!search.replace_mode);

        let mut scene = Scene::new(wgpu::Color::BLACK);
        search.render(&mut scene, (800.0, 600.0), 8.0, &editor);
        let layout = search.last.unwrap();
        assert_eq!(layout.replace_field.w, 0.0, "sin fila de reemplazo");
        // fondo + borde + campo + caret.
        assert_eq!(scene.top_quads.len(), 4);

        // Ctrl+H no escribe en la barra: ese atajo lo resuelve el keymap
        // ANTES de repartir el teclado (aquí solo caería en `_`).
        search.handle_key(&key('h'), ModifiersState::CONTROL, &mut editor);
        assert!(search.query.is_empty(), "Ctrl+H no escribe 'h'");
        assert!(search.is_open(), "la barra no se cierra sola");
    }
}
