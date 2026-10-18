//! Buscar en archivos (Ctrl+Shift+F) con panel de resultados al pie.
//!
//! El panel ocupa el borde inferior sobre la barra de estado: una fila con
//! la consulta y, debajo, las coincidencias como `ruta:12: texto`. La
//! búsqueda es incremental con Enter (smart-case, límites de resultados y
//! de tamaño de archivo para no colgar); Enter sobre resultados existentes
//! abre el seleccionado y Esc cierra. La resuelve `main.rs`.

use std::path::PathBuf;

use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::editor::FONT_SIZE;
use crate::theme::Theme;

/// Altura de la fila de consulta (px lógicos).
const QUERY_H: f32 = 30.0;
/// Altura de cada fila de resultado.
const ROW_H: f32 = 20.0;
/// Filas de resultados visibles como máximo.
const MAX_ROWS: usize = 12;
/// Tope de coincidencias recogidas por búsqueda.
const MAX_RESULTS: usize = 1_000;
/// Archivos mayores que esto se saltan (no son texto útil).
const MAX_FILE: u64 = 1_048_576;
/// Margen horizontal dentro del panel.
const PAD: f32 = 8.0;
/// Franja de borde superior del panel.
const EDGE: f32 = 1.0;

/// Rectángulo para el hit-test del ratón.
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

/// Qué debe hacer `main.rs` tras [`FindFiles::on_key`].
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Abrir este archivo en la línea dada.
    Open(PathBuf, usize),
}

/// Qué toca el ratón dentro del panel ([`FindFiles::click`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    /// Fuera del panel: se cierra y el click sigue (editor/divisor).
    Miss,
    /// Dentro pero sin fila de resultado: se traga.
    Panel,
    /// Una fila: abrir este archivo en esta línea.
    Row(PathBuf, usize),
}

/// Una coincidencia: archivo, línea (1-based) y su texto recortado.
struct Match {
    path: PathBuf,
    line: usize,
    text: String,
}

/// Layout del último render (para [`FindFiles::click`]).
#[derive(Clone, Copy)]
struct Layout {
    panel: Rect,
    query: Rect,
    /// Y donde empiezan las filas visibles de resultados.
    rows_y: f32,
    /// Filas visibles dibujadas.
    rows: usize,
}

pub struct FindFiles {
    open: bool,
    query: String,
    /// Resultados del último [`FindFiles::run_search`].
    results: Vec<Match>,
    selected: usize,
    scroll: usize,
    /// Query de la última búsqueda (`None` = resultados desactualizados).
    searched: Option<String>,
    /// Raíz del recorrido y árbol de archivos (los recoge `main` al abrir).
    root: PathBuf,
    files: Vec<PathBuf>,
    last: Option<Layout>,
    /// Avance (ancho de glifo) del último render, para truncar filas.
    advance: f32,
}

impl Default for FindFiles {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            searched: None,
            root: PathBuf::new(),
            files: Vec::new(),
            last: None,
            advance: 8.0,
        }
    }
}

impl FindFiles {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Abre el panel con `prefill` (selección de UNA línea) como consulta.
    /// Si la consulta no está vacía, busca al instante sobre `files`.
    pub fn open(&mut self, prefill: Option<&str>, root: PathBuf, files: Vec<PathBuf>) {
        self.open = true;
        self.root = root;
        self.files = files;
        if let Some(text) = prefill {
            self.query = text.to_owned();
        }
        if !self.query.is_empty() {
            self.run_search();
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.last = None;
    }

    /// Texto del IME dentro del panel abierto.
    pub fn push_text(&mut self, text: &str) {
        if !self.open || text.is_empty() {
            return;
        }
        self.query.push_str(text);
        self.searched = None;
    }

    /// Teclado del panel (solo mientras está abierto).
    pub fn on_key(&mut self, event: &KeyEvent, modifiers: ModifiersState) -> Action {
        if event.state == ElementState::Released {
            return Action::None;
        }
        self.handle_key(&event.logical_key, modifiers)
    }

    /// Núcleo de [`FindFiles::on_key`] (separable para tests: `KeyEvent` no
    /// es construible fuera de winit).
    fn handle_key(&mut self, key: &Key, modifiers: ModifiersState) -> Action {
        if !self.open {
            return Action::None;
        }
        match key {
            Key::Named(NamedKey::Escape) => Action::Close,
            Key::Named(NamedKey::Enter) => {
                if self.searched.as_deref() != Some(self.query.as_str()) {
                    // Query nueva: (re)buscar antes de saltar.
                    self.run_search();
                    Action::None
                } else {
                    match self.results.get(self.selected) {
                        Some(found) => Action::Open(found.path.clone(), found.line),
                        None => Action::None,
                    }
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.query.pop().is_some() {
                    self.searched = None;
                }
                Action::None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_selection(-1);
                Action::None
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_selection(1);
                Action::None
            }
            // Imprimibles: salvo Ctrl/Cmd puro; AltGr (Ctrl+Alt) sí escribe.
            Key::Character(text)
                if !(modifiers.control_key() || modifiers.super_key()) || modifiers.alt_key() =>
            {
                self.query.push_str(text);
                self.searched = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let max = self.results.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max as isize) as usize;
        // La selección se mantiene visible dentro del scroll.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + MAX_ROWS {
            self.scroll = self.selected + 1 - MAX_ROWS;
        }
    }

    /// Recorre `files` buscando `query` (smart-case: mayúsculas en la
    /// consulta activan la distinción). Recorta a [`MAX_RESULTS`], salta
    /// binarios (NUL en la cabecera) y archivos mayores que [`MAX_FILE`].
    fn run_search(&mut self) {
        self.searched = Some(self.query.clone());
        self.results.clear();
        self.selected = 0;
        self.scroll = 0;
        if self.query.is_empty() {
            return;
        }
        let case_sensitive = self.query.chars().any(char::is_uppercase);
        let needle = if case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };
        'files: for path in &self.files {
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            if meta.len() > MAX_FILE {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if bytes.starts_with(b"\0") {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                let hit = if case_sensitive {
                    line.contains(&needle)
                } else {
                    line.to_lowercase().contains(&needle)
                };
                if hit {
                    let mut line_text = line.trim().to_owned();
                    if line_text.chars().count() > 200 {
                        line_text.truncate(200);
                        line_text.push('…');
                    }
                    self.results.push(Match {
                        path: path.clone(),
                        line: index + 1,
                        text: line_text,
                    });
                    if self.results.len() >= MAX_RESULTS {
                        break 'files;
                    }
                }
            }
        }
    }

    /// Dibuja el panel sobre la franja inferior reservada (`bottom` = alto
    /// de la barra de estado) y guarda el layout para [`FindFiles::click`].
    pub fn render(
        &mut self,
        scene: &mut Scene,
        viewport: (f32, f32),
        advance: f32,
        bottom: f32,
        theme: &Theme,
    ) {
        if !self.open {
            self.last = None;
            return;
        }
        self.advance = advance;
        let visible = self.results.len().saturating_sub(self.scroll).min(MAX_ROWS);
        let panel_h = EDGE + QUERY_H + visible as f32 * ROW_H;
        let panel_y = (viewport.1 - bottom - panel_h).max(0.0);
        let panel = Rect::new(0.0, panel_y, viewport.0, panel_h);
        scene.top_quads.push(Quad::new(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            theme.panel_bg,
        ));
        scene
            .top_quads
            .push(Quad::new(0.0, panel_y, viewport.0, EDGE, theme.panel_edge));

        // Fila de consulta: etiqueta, campo con caret al final y contador.
        let row_y = panel_y + EDGE;
        let label = "Buscar en archivos:";
        let field_x = PAD + label.chars().count() as f32 * advance + 6.0;
        let counter = if self.searched.as_deref() != Some(self.query.as_str()) {
            "Enter para buscar".to_owned()
        } else if self.results.len() >= MAX_RESULTS {
            format!("{}+", MAX_RESULTS)
        } else {
            let n = self.results.len();
            format!("{n} {}", if n == 1 { "resultado" } else { "resultados" })
        };
        let counter_w = counter.chars().count() as f32 * advance;
        let field = Rect::new(
            field_x,
            row_y + 4.0,
            (viewport.0 - field_x - PAD - counter_w - 8.0).max(60.0),
            QUERY_H - 8.0,
        );
        scene.top_texts.push(TextSpan::new(
            label,
            PAD,
            row_y + (QUERY_H - FONT_SIZE) / 2.0,
            FONT_SIZE,
            theme.fg,
        ));
        push_field(scene, field, &self.query, advance, theme);
        scene.top_texts.push(TextSpan::new(
            counter,
            viewport.0 - PAD - counter_w,
            row_y + (QUERY_H - FONT_SIZE) / 2.0,
            FONT_SIZE,
            theme.dim,
        ));

        // Filas de resultados visibles: `ruta:12: texto`.
        let rows_y = row_y + QUERY_H;
        let capacity = ((viewport.0 - 2.0 * PAD) / advance).floor().max(4.0) as usize;
        for (row, found) in self
            .results
            .iter()
            .skip(self.scroll)
            .take(MAX_ROWS)
            .enumerate()
        {
            let y = rows_y + row as f32 * ROW_H;
            let is_selected = self.scroll + row == self.selected;
            if is_selected {
                scene
                    .top_quads
                    .push(Quad::new(0.0, y, viewport.0, ROW_H, theme.tab_active_bg));
            }
            let rel = found
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&found.path)
                .to_string_lossy()
                .replace('\\', "/");
            let mut label = format!("{rel}:{}: {}", found.line, found.text);
            if label.chars().count() > capacity {
                let head: String = label.chars().take(capacity.saturating_sub(1)).collect();
                label = format!("{head}\u{2026}");
            }
            scene.top_texts.push(TextSpan::new(
                label,
                PAD,
                y + (ROW_H - FONT_SIZE) / 2.0,
                FONT_SIZE,
                theme.fg,
            ));
        }

        self.last = Some(Layout {
            panel,
            query: field,
            rows_y,
            rows: visible,
        });
    }

    /// Click dentro del panel abierta (usa el layout del último render).
    pub fn click(&self, x: f32, y: f32) -> Click {
        let Some(layout) = self.last else {
            return Click::Miss; // aún sin render: se traga el primer click
        };
        if !layout.panel.contains(x, y) {
            return Click::Miss;
        }
        if layout.query.contains(x, y) {
            return Click::Panel;
        }
        let row = ((y - layout.rows_y) / ROW_H).floor() as usize;
        if row < layout.rows {
            let index = self.scroll + row;
            if let Some(found) = self.results.get(index) {
                return Click::Row(found.path.clone(), found.line);
            }
        }
        Click::Panel
    }
}

/// Campo de texto con fondo y caret al final (mismo estilo que la paleta).
fn push_field(scene: &mut Scene, rect: Rect, text: &str, advance: f32, theme: &Theme) {
    scene
        .top_quads
        .push(Quad::new(rect.x, rect.y, rect.w, rect.h, theme.tab_bg));
    let y = rect.y + (rect.h - FONT_SIZE) / 2.0;
    scene
        .top_texts
        .push(TextSpan::new(text, rect.x + 4.0, y, FONT_SIZE, theme.fg));
    let caret_x = rect.x + 4.0 + text.chars().count() as f32 * advance;
    if caret_x < rect.x + rect.w - 2.0 {
        scene.top_quads.push(Quad::new(
            caret_x,
            rect.y + 3.0,
            1.0,
            rect.h - 6.0,
            theme.caret,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scene() -> Scene {
        Scene::new(wgpu::Color::BLACK)
    }

    /// Escribe `content` en `dir/name` y devuelve la ruta.
    fn write(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).expect("crea");
        file.write_all(content.as_bytes()).expect("escribe");
        path
    }

    /// Panel abierto sobre un directorio con dos archivos.
    fn open_over(dir: &tempfile::TempDir) -> (FindFiles, PathBuf, PathBuf) {
        let a = write(dir, "a.rs", "fn main() {\n    let mundo = 1;\n}\n");
        let b = write(dir, "b.txt", "hola mundo\nadios\n");
        let mut find = FindFiles::default();
        find.open(None, dir.path().to_path_buf(), vec![a.clone(), b.clone()]);
        (find, a, b)
    }

    #[test]
    fn prefill_searches_immediately_and_jumps_to_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut find, a, _b) = open_over(&dir);
        find.open(Some("mundo"), dir.path().to_path_buf(), find.files.clone());
        assert_eq!(find.results.len(), 2, "mundo en a.rs y b.txt");
        // searched == query tras open → Enter abre el seleccionado (0).
        let found = &find.results[find.selected];
        assert_eq!(found.path, a, "el recorrido empieza por a.rs");
        assert_eq!(found.line, 2);
        let action = find.handle_key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
        assert_eq!(action, Action::Open(a, 2));
    }

    #[test]
    fn smart_case_sensitizes_only_with_uppercase() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir, "x.txt", "Mundo mayuscula\nmundo minuscula\n");
        let path = dir.path().join("x.txt");
        let mut find = FindFiles::default();
        find.open(None, dir.path().to_path_buf(), vec![path.clone()]);

        find.query = "mundo".to_owned();
        find.run_search();
        assert_eq!(find.results.len(), 2, "minúsculas: indistinguible");

        find.query = "Mundo".to_owned();
        find.run_search();
        assert_eq!(find.results.len(), 1, "con mayúscula: sensible");
        assert_eq!(find.results[0].line, 1);
    }

    #[test]
    fn stale_query_searches_on_first_enter_then_opens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut find, a, _b) = open_over(&dir);
        find.push_text("fn main");
        let action = find.handle_key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
        assert_eq!(action, Action::None, "primer Enter busca");
        assert_eq!(find.results.len(), 1);
        let action = find.handle_key(&Key::Named(NamedKey::Enter), ModifiersState::empty());
        assert_eq!(action, Action::Open(a, 1), "segundo Enter salta");
    }

    #[test]
    fn keys_edit_query_and_escape_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut find, _, _) = open_over(&dir);
        find.query.clear();
        find.searched = None;
        find.handle_key(&Key::Character("ho".into()), ModifiersState::empty());
        assert_eq!(find.query, "ho");
        find.handle_key(&Key::Named(NamedKey::Backspace), ModifiersState::empty());
        assert_eq!(find.query, "h");
        // Con ctrl el texto no se escribe (atajos).
        find.handle_key(&Key::Character("z".into()), ModifiersState::CONTROL);
        assert_eq!(find.query, "h");
        let action = find.handle_key(&Key::Named(NamedKey::Escape), ModifiersState::empty());
        assert_eq!(action, Action::Close, "Esc pide cerrar (lo cierra main)");
        find.close();
        assert!(!find.is_open());
        // Cerrado, el panel ignora el teclado.
        let action = find.handle_key(&Key::Character("x".into()), ModifiersState::empty());
        assert_eq!(action, Action::None);
    }

    #[test]
    fn arrows_move_selection_and_scroll_follows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content: String = (0..30).map(|i| format!("hola linea {i}\n")).collect();
        write(&dir, "big.txt", &content);
        let mut find = FindFiles::default();
        find.open(
            None,
            dir.path().to_path_buf(),
            vec![dir.path().join("big.txt")],
        );
        find.query = "hola".to_owned();
        find.run_search();
        assert_eq!(find.results.len(), 30);
        for _ in 0..15 {
            find.handle_key(&Key::Named(NamedKey::ArrowDown), ModifiersState::empty());
        }
        assert_eq!(find.selected, 15);
        assert_eq!(find.scroll, 15 + 1 - MAX_ROWS, "la selección queda visible");
        find.handle_key(&Key::Named(NamedKey::ArrowUp), ModifiersState::empty());
        assert_eq!(find.selected, 14);
    }

    #[test]
    fn render_stores_layout_and_click_opens_the_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut find, a, b) = open_over(&dir);
        find.query = "mundo".to_owned();
        find.run_search();
        let mut s = scene();
        find.render(&mut s, (800.0, 600.0), 8.0, 24.0, &Theme::dark());
        let layout = find.last.expect("render guarda el layout");
        // El panel queda justo encima de la franja inferior de 24 px.
        assert!((layout.panel.y + layout.panel.h - (600.0 - 24.0)).abs() < 0.01);
        assert_eq!(layout.rows, 2, "dos coincidencias");
        // Primera fila (a.rs) → abrir a.rs en su línea.
        let y = layout.rows_y + ROW_H / 2.0;
        assert_eq!(find.click(100.0, y), Click::Row(a, 2));
        let y2 = layout.rows_y + ROW_H + ROW_H / 2.0;
        assert_eq!(find.click(100.0, y2), Click::Row(b, 1));
        // Fuera del panel: Miss (cierra); en la consulta: Panel.
        assert_eq!(find.click(100.0, 590.0), Click::Miss);
        assert_eq!(
            find.click(layout.query.x + 4.0, layout.query.y + 4.0),
            Click::Panel
        );
    }

    #[test]
    fn skips_binary_and_huge_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binario = write(&dir, "bin.dat", "\0\0\0buscar texto");
        let huge = write(&dir, "huge.log", "");
        let file = std::fs::File::options()
            .write(true)
            .open(&huge)
            .expect("abre huge");
        file.set_len(MAX_FILE + 1).expect("crece");
        let normal = write(&dir, "ok.txt", "buscar texto\n");
        let mut find = FindFiles::default();
        find.open(
            None,
            dir.path().to_path_buf(),
            vec![binario, huge, normal.clone()],
        );
        find.query = "buscar".to_owned();
        find.run_search();
        assert_eq!(find.results.len(), 1, "solo el archivo de texto normal");
        assert_eq!(find.results[0].path, normal);
    }

    #[test]
    fn results_are_capped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = "coincidencia\n".repeat(MAX_RESULTS + 10);
        write(&dir, "cap.txt", &content);
        let mut find = FindFiles::default();
        find.open(
            None,
            dir.path().to_path_buf(),
            vec![dir.path().join("cap.txt")],
        );
        find.query = "coincidencia".to_owned();
        find.run_search();
        assert_eq!(find.results.len(), MAX_RESULTS, "tope respetado");
    }
}
