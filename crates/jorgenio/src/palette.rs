//! Paleta de comandos (Ctrl+Shift+P) y búsqueda rápida de archivos
//! (Ctrl+P): fuzzy matching con `nucleo-matcher`, dibujada en la capa
//! superior de la [`Scene`] (tras todo el contenido).
//!
//! La resuelve `main.rs`: [`Action::Run`] trae el id del comando ejecutable
//! y [`Action::Open`] el archivo elegido en el modo de archivos.

use std::path::{Path, PathBuf};

use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::commands::{self, COMMANDS};
use crate::editor::FONT_SIZE;

/// Altura de fila de resultados (px lógicos).
const ROW_H: f32 = 22.0;
/// Altura de la fila de prompt (px lógicos).
const PROMPT_H: f32 = 30.0;
/// Distancia desde el borde superior de la ventana (px lógicos).
const TOP: f32 = 48.0;
/// Margen lateral respecto al borde de la ventana (px lógicos).
const PAD: f32 = 16.0;
/// Filas visibles como máximo.
const MAX_ROWS: usize = 10;
/// Ancho máximo del panel (px lógicos).
const MAX_W: f32 = 560.0;
/// Ancho mínimo del panel (px lógicos).
const MIN_W: f32 = 200.0;

const PANEL_BG: (u8, u8, u8, u8) = (0x1E, 0x1F, 0x1C, 0xFF);
const PANEL_EDGE: (u8, u8, u8, u8) = (0x3E, 0x3D, 0x32, 0xFF);
const HIGHLIGHT: (u8, u8, u8, u8) = (0x49, 0x48, 0x3E, 0xFF);
const FG: (u8, u8, u8) = (0xF8, 0xF8, 0xF2);
const DIM: (u8, u8, u8) = (0x75, 0x71, 0x5E);
/// Caret del prompt (mismo blanco que FG, pero con alfa para los quads).
const CARET: (u8, u8, u8, u8) = (0xF8, 0xF8, 0xF2, 0xFF);

/// Qué debe hacer `main.rs` tras [`Palette::on_key`].
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Ejecutar el comando con este id.
    Run(&'static str),
    /// Abrir este archivo (modo rápido, Ctrl+P).
    Open(PathBuf),
}

/// Qué toca el ratón dentro de la paleta abierta ([`Palette::click`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    /// Fuera del panel: se cierra y el click sigue hacia el editor.
    Miss,
    /// Dentro pero sin fila de comando (prompt o zona vacía): se traga.
    Panel,
    /// Una fila de resultados: ejecutar este comando.
    Command(&'static str),
    /// Una fila de archivos: abrir este archivo.
    File(PathBuf),
}

/// Qué lista está puntuando la paleta.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Commands,
    Files,
}

/// Geometría del panel del último render (para [`Palette::click`]).
#[derive(Clone, Copy)]
struct PanelRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Y donde empiezan las filas de resultados.
    rows_y: f32,
    /// Filas de resultados visibles (no confundir con la de "(sin…)".
    rows: usize,
}

pub struct Palette {
    open: bool,
    query: String,
    /// Índice dentro de `matches` (no de la lista fuente).
    selected: usize,
    scroll: usize,
    /// Índices de la lista fuente que puntúan, de mayor a menor.
    matches: Vec<usize>,
    matcher: Matcher,
    panel: Option<PanelRect>,
    /// Lista fuente: comandos (estáticos) o archivos (del último Ctrl+P).
    mode: Mode,
    /// Raíz del Ctrl+P (para rutas relativas en pantalla y en el match).
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            scroll: 0,
            matches: (0..COMMANDS.len()).collect(),
            matcher: Matcher::new(Config::DEFAULT),
            panel: None,
            mode: Mode::Commands,
            root: PathBuf::new(),
            files: Vec::new(),
        }
    }
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Abre en modo comandos (Ctrl+Shift+P): muestra todos los comandos.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.mode = Mode::Commands;
        self.root.clear();
        self.files.clear();
        self.refilter();
    }

    /// Abre en modo archivos (Ctrl+P) sobre `root` con el árbol ya recogido.
    pub fn open_files(&mut self, root: PathBuf, files: Vec<PathBuf>) {
        self.open = true;
        self.query.clear();
        self.mode = Mode::Files;
        self.root = root;
        self.files = files;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.panel = None;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Texto del IME dentro de la paleta abierta.
    pub fn push_text(&mut self, text: &str) {
        if !self.open || text.is_empty() {
            return;
        }
        self.query.push_str(text);
        self.refilter();
    }

    /// Re-evalúa el fuzzy match sobre `query` y reordena por puntuación
    /// (sort estable: los empates conservan el orden de la lista fuente).
    fn refilter(&mut self) {
        let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(usize, u32)> = match self.mode {
            Mode::Commands => COMMANDS
                .iter()
                .enumerate()
                .filter_map(|(index, command)| {
                    let hay = format!("{} {} {}", command.name, command.category, command.id);
                    let hay = Utf32String::from(hay.as_str());
                    pattern
                        .score(hay.slice(..), &mut self.matcher)
                        .map(|score| (index, score))
                })
                .collect(),
            Mode::Files => {
                // Las etiquetas se materializan antes: `matcher` pide &mut.
                let labels: Vec<String> = self
                    .files
                    .iter()
                    .map(|path| rel_label(&self.root, path))
                    .collect();
                labels
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, hay)| {
                        let hay = Utf32String::from(hay.as_str());
                        pattern
                            .score(hay.slice(..), &mut self.matcher)
                            .map(|score| (index, score))
                    })
                    .collect()
            }
        };
        scored.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
        self.matches = scored.into_iter().map(|(index, _)| index).collect();
        self.selected = 0;
        self.scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let max = self.matches.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max as isize) as usize;
    }

    /// Teclado de la paleta (solo mientras está abierta).
    pub fn on_key(&mut self, event: &KeyEvent, modifiers: ModifiersState) -> Action {
        if event.state == ElementState::Released {
            return Action::None;
        }
        self.handle_key(&event.logical_key, modifiers)
    }

    /// Núcleo de [`Palette::on_key`] (separable para tests: `KeyEvent` no es
    /// construible fuera de winit).
    fn handle_key(&mut self, key: &Key, modifiers: ModifiersState) -> Action {
        if !self.open {
            return Action::None;
        }
        match key {
            Key::Named(NamedKey::Escape) => Action::Close,
            Key::Named(NamedKey::Enter) => match self.matches.get(self.selected) {
                Some(&index) => match self.mode {
                    Mode::Commands => Action::Run(COMMANDS[index].id),
                    Mode::Files => Action::Open(self.files[index].clone()),
                },
                None => Action::Close,
            },
            Key::Named(NamedKey::Backspace) => {
                if self.query.pop().is_some() {
                    self.refilter();
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
                self.refilter();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Qué toca el ratón dentro de la paleta (usa el layout del último
    /// render). `main` cierra siempre y solo ejecuta con [`Click::Command`].
    pub fn click(&self, x: f32, y: f32) -> Click {
        let Some(panel) = self.panel else {
            return Click::Miss;
        };
        if x < panel.x || x > panel.x + panel.w || y < panel.y || y > panel.y + panel.h {
            return Click::Miss;
        }
        if y < panel.rows_y {
            return Click::Panel;
        }
        let row = ((y - panel.rows_y) / ROW_H).floor() as usize;
        let index = row + self.scroll;
        if row < panel.rows && index < self.matches.len() {
            match self.mode {
                Mode::Commands => Click::Command(COMMANDS[self.matches[index]].id),
                Mode::Files => Click::File(self.files[self.matches[index]].clone()),
            }
        } else {
            Click::Panel
        }
    }

    /// Dibuja el panel en la capa superior (si está abierta). `advance` es el
    /// ancho de celda (px lógicos), igual que en el editor.
    pub fn render(&mut self, scene: &mut Scene, viewport: (f32, f32), advance: f32) {
        if !self.open {
            return;
        }

        let rows = self.matches.len().clamp(1, MAX_ROWS);
        // Que la selección siempre quede visible.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected - rows + 1;
        }

        let width = (viewport.0 - 2.0 * PAD).clamp(MIN_W, MAX_W);
        let x = (viewport.0 - width) / 2.0;
        let height = PROMPT_H + 1.0 + rows as f32 * ROW_H + 8.0;
        self.panel = Some(PanelRect {
            x,
            y: TOP,
            w: width,
            h: height,
            rows_y: TOP + PROMPT_H + 1.0,
            rows: self.matches.len().min(MAX_ROWS),
        });

        scene
            .top_quads
            .push(Quad::new(x, TOP, width, height, PANEL_BG));
        scene
            .top_quads
            .push(Quad::new(x, TOP, width, 1.0, PANEL_EDGE));
        scene
            .top_quads
            .push(Quad::new(x, TOP + height - 1.0, width, 1.0, PANEL_EDGE));
        scene
            .top_quads
            .push(Quad::new(x, TOP + PROMPT_H, width, 1.0, PANEL_EDGE));

        // Prompt "> query" + caret fijo (2 px, siempre visible).
        let prompt_x = x + 8.0;
        scene.top_texts.push(TextSpan::new(
            format!("> {}", self.query),
            prompt_x,
            TOP + (PROMPT_H - FONT_SIZE) / 2.0,
            FONT_SIZE,
            FG,
        ));
        let caret_x = prompt_x + (2 + self.query.chars().count()) as f32 * advance;
        scene
            .top_quads
            .push(Quad::new(caret_x, TOP + 7.0, 2.0, PROMPT_H - 14.0, CARET));

        if self.matches.is_empty() {
            scene.top_texts.push(TextSpan::new(
                "(sin coincidencias)",
                prompt_x,
                TOP + PROMPT_H + 1.0 + (ROW_H - FONT_SIZE) / 2.0,
                FONT_SIZE,
                DIM,
            ));
            return;
        }

        // Fila resaltada (bajo los textos: se dibuja antes que ellos).
        let highlight_row = self.selected - self.scroll;
        let highlight_y = TOP + PROMPT_H + 1.0 + highlight_row as f32 * ROW_H;
        scene
            .top_quads
            .push(Quad::new(x, highlight_y, width, ROW_H, HIGHLIGHT));

        for (visible, &index) in self.matches.iter().skip(self.scroll).take(rows).enumerate() {
            let y = TOP + PROMPT_H + 1.0 + visible as f32 * ROW_H + (ROW_H - FONT_SIZE) / 2.0;
            match self.mode {
                Mode::Commands => {
                    let command = &COMMANDS[index];
                    scene
                        .top_texts
                        .push(TextSpan::new(command.name, x + 8.0, y, FONT_SIZE, FG));
                    // Categoría + atajo, alineados a la derecha; se omiten si pisan
                    // el nombre (viewport muy estrecho).
                    let right = match commands::keys_for(command.id) {
                        Some(keys) => format!("{}  {keys}", command.category),
                        None => command.category.to_string(),
                    };
                    let right_x = x + width - 8.0 - right.chars().count() as f32 * advance;
                    if right_x > x + 8.0 + command.name.chars().count() as f32 * advance + 4.0 {
                        scene
                            .top_texts
                            .push(TextSpan::new(right, right_x, y, FONT_SIZE, DIM));
                    }
                }
                Mode::Files => {
                    // Ruta relativa; si no cabe, se corta por la izquierda
                    // (conservando el nombre del archivo).
                    let label = rel_label(&self.root, &self.files[index]);
                    let max_chars = ((width - 16.0) / advance).floor().max(1.0) as usize;
                    let label = if label.chars().count() > max_chars {
                        let skip = label.chars().count() - max_chars + 1;
                        format!("…{}", label.chars().skip(skip).collect::<String>())
                    } else {
                        label
                    };
                    scene
                        .top_texts
                        .push(TextSpan::new(label, x + 8.0, y, FONT_SIZE, FG));
                }
            }
        }
    }
}

/// Ruta relativa a `root` con `/` (para mostrarla y puntuarla igual que
/// la teclea el usuario). Fuera de `root` se queda con la ruta completa.
fn rel_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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

    fn match_ids(palette: &Palette) -> Vec<&'static str> {
        palette.matches.iter().map(|&i| COMMANDS[i].id).collect()
    }

    #[test]
    fn open_close_toggle_reset_query() {
        let mut palette = Palette::default();
        assert!(!palette.is_open());
        palette.open();
        assert!(palette.is_open());
        palette.push_text("guardar");
        assert_eq!(palette.query, "guardar");
        palette.toggle();
        assert!(!palette.is_open());
        palette.toggle();
        assert!(palette.is_open());
        assert!(palette.query.is_empty(), "reabrir limpia la query");
        assert_eq!(palette.matches.len(), COMMANDS.len());
    }

    #[test]
    fn refilter_fuzzy_orders_and_filters() {
        let mut palette = Palette::default();
        palette.open();

        palette.push_text("línea");
        let ids = match_ids(&palette);
        assert_eq!(ids.len(), 6, "solo comandos de línea: {ids:?}");
        assert!(ids.contains(&"edit.delete_line"));
        assert!(ids.contains(&"edit.duplicate_line"));
        assert!(ids.contains(&"edit.join_lines"));
        assert!(ids.contains(&"edit.expand_selection_line"));
        assert!(ids.contains(&"edit.move_line_up"));
        assert!(ids.contains(&"edit.move_line_down"));

        palette.push_text("xyz");
        assert!(palette.matches.is_empty(), "query imposible");

        // Backspace hasta "línea" otra vez: re-filtra y recuperan los 6.
        for _ in 0..3 {
            palette.handle_key(&Key::Named(NamedKey::Backspace), mods());
        }
        assert_eq!(palette.query, "línea");
        assert_eq!(palette.matches.len(), 6, "backspace re-filtra");
    }

    #[test]
    fn enter_runs_selected_escape_closes() {
        let mut palette = Palette::default();
        palette.open();

        // La primera fila del estado por defecto es "palette.show".
        let action = palette.handle_key(&Key::Named(NamedKey::Enter), mods());
        assert_eq!(action, Action::Run("palette.show"));

        let action = palette.handle_key(&Key::Named(NamedKey::Escape), mods());
        assert_eq!(action, Action::Close);
    }

    #[test]
    fn arrows_move_selection_with_clamp() {
        let mut palette = Palette::default();
        palette.open();
        palette.handle_key(&Key::Named(NamedKey::ArrowDown), mods());
        palette.handle_key(&Key::Named(NamedKey::ArrowDown), mods());
        assert_eq!(palette.selected, 2);
        palette.handle_key(&Key::Named(NamedKey::ArrowUp), mods());
        assert_eq!(palette.selected, 1);
        for _ in 0..50 {
            palette.handle_key(&Key::Named(NamedKey::ArrowUp), mods());
        }
        assert_eq!(palette.selected, 0, "no baja de 0");
        for _ in 0..100 {
            palette.handle_key(&Key::Named(NamedKey::ArrowDown), mods());
        }
        assert_eq!(palette.selected, COMMANDS.len() - 1, "no pasa del final");
    }

    #[test]
    fn ctrl_characters_do_not_type_altgr_does() {
        let mut palette = Palette::default();
        palette.open();
        let mut ctrl = ModifiersState::CONTROL;
        assert_eq!(palette.handle_key(&key('s'), ctrl), Action::None);
        assert!(palette.query.is_empty(), "Ctrl+S no escribe");

        ctrl.insert(ModifiersState::ALT);
        assert_eq!(palette.handle_key(&key('é'), ctrl), Action::None);
        assert_eq!(palette.query, "é", "AltGr sí escribe");

        assert_eq!(
            palette.handle_key(&key('p'), ModifiersState::empty()),
            Action::None
        );
        assert_eq!(palette.query, "ép");
    }

    #[test]
    fn render_pushes_top_layer_only_when_open() {
        let mut palette = Palette::default();
        let mut scene = Scene::new(wgpu::Color::BLACK);
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        assert!(scene.top_quads.is_empty() && scene.top_texts.is_empty());

        palette.open();
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        assert!(!scene.top_quads.is_empty(), "fondo + bordes + caret");
        assert!(!scene.top_texts.is_empty(), "prompt + filas");
        // prompt + hasta 10 filas con nombre y etiqueta a la derecha.
        assert!(scene.top_texts.len() <= 1 + 2 * MAX_ROWS);

        palette.push_text("xyz");
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        let empty_texts: Vec<_> = scene
            .top_texts
            .iter()
            .filter(|span| span.text.contains("sin coincidencias"))
            .collect();
        assert!(!empty_texts.is_empty(), "muestra aviso sin coincidencias");
    }

    #[test]
    fn render_handles_narrow_viewport() {
        let mut palette = Palette::default();
        palette.open();
        let mut scene = Scene::new(wgpu::Color::BLACK);
        palette.render(&mut scene, (120.0, 800.0), 8.4);
        assert!(!scene.top_quads.is_empty());
        assert!(!scene.top_texts.is_empty());
    }

    #[test]
    fn click_runs_row_reports_panel_or_miss() {
        let mut palette = Palette::default();
        assert_eq!(
            palette.click(0.0, 0.0),
            Click::Miss,
            "sin render no hay layout"
        );

        palette.open();
        let mut scene = Scene::new(wgpu::Color::BLACK);
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        let panel = palette.panel.expect("render guarda el panel");

        // Filas 0 y 1 del orden por defecto (= orden de COMMANDS).
        assert_eq!(
            palette.click(panel.x + 10.0, panel.rows_y + 5.0),
            Click::Command("palette.show")
        );
        assert_eq!(
            palette.click(panel.x + 10.0, panel.rows_y + ROW_H + 5.0),
            Click::Command("file.save")
        );
        // Prompt (encima de las filas): dentro del panel, sin comando.
        assert_eq!(palette.click(panel.x + 10.0, panel.y + 5.0), Click::Panel);
        // Fuera del panel: main lo cierra y el click sigue al editor.
        assert_eq!(palette.click(0.0, 0.0), Click::Miss);

        // Con query filtrada, el índice de fila mapea por matches, no por COMMANDS.
        palette.push_text("línea");
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        let expected = COMMANDS[palette.matches[0]].id;
        assert_eq!(
            palette.click(panel.x + 10.0, panel.rows_y + 5.0),
            Click::Command(expected)
        );

        palette.close();
        assert_eq!(
            palette.click(panel.x + 10.0, panel.rows_y + 5.0),
            Click::Miss
        );
    }

    #[test]
    fn open_files_matches_relative_paths_with_slashes() {
        let mut palette = Palette::default();
        palette.open_files(
            std::path::PathBuf::from("D:\\p"),
            vec![
                std::path::PathBuf::from("D:\\p\\src\\main.rs"),
                std::path::PathBuf::from("D:\\p\\README.md"),
                std::path::PathBuf::from("D:\\p\\target\\gen.rs"),
            ],
        );
        assert!(palette.is_open());
        assert_eq!(palette.matches.len(), 3, "query vacía: todos");

        // La etiqueta usa '/', así que se teclea con '/'.
        palette.push_text("src/ma");
        assert_eq!(palette.matches.len(), 1, "solo el fuente");
        let action = palette.handle_key(&Key::Named(NamedKey::Enter), mods());
        assert_eq!(
            action,
            Action::Open(std::path::PathBuf::from("D:\\p\\src\\main.rs")),
            "Enter abre el archivo elegido"
        );

        // Abrir en modo comandos reactiva la lista de comandos.
        palette.open();
        assert_eq!(palette.matches.len(), COMMANDS.len());
        assert_eq!(palette.files.len(), 0);
    }

    #[test]
    fn files_mode_click_returns_the_path() {
        let mut palette = Palette::default();
        palette.open_files(
            std::path::PathBuf::from("D:\\p"),
            vec![std::path::PathBuf::from("D:\\p\\a.rs")],
        );
        let mut scene = Scene::new(wgpu::Color::BLACK);
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        let panel = palette.panel.expect("render guarda el panel");
        assert_eq!(
            palette.click(panel.x + 10.0, panel.rows_y + 5.0),
            Click::File(std::path::PathBuf::from("D:\\p\\a.rs"))
        );
        // La fila pinta la ruta relativa, con '/' y sin el prefijo del root.
        assert!(scene.top_texts.iter().any(|span| span.text == "a.rs"));
    }

    #[test]
    fn open_files_with_no_candidates_shows_the_empty_hint() {
        let mut palette = Palette::default();
        palette.open_files(std::path::PathBuf::from("D:\\p"), Vec::new());
        assert!(palette.matches.is_empty());
        let mut scene = Scene::new(wgpu::Color::BLACK);
        palette.render(&mut scene, (1200.0, 800.0), 8.4);
        assert!(
            scene
                .top_texts
                .iter()
                .any(|span| span.text.contains("sin coincidencias"))
        );
        assert_eq!(
            palette.handle_key(&Key::Named(NamedKey::Enter), mods()),
            Action::Close
        );
    }
}
