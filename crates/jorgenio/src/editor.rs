//! Editor de texto: estado de un buffer + caret + scroll + construcción de escena.

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use jorgenio_core::buffer::Buffer;
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

pub struct Editor {
    buffer: Buffer,
    path: Option<PathBuf>,
    /// Offset del caret en **chars** (nativo de ropey).
    caret: usize,
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
            path: None,
            caret: 0,
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

    /// Teclado (físico/lógico): insere, borra y mueve. `true` si pide redraw.
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
                let caret = movement::prev_grapheme(&self.buffer, self.caret);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                let caret = movement::next_grapheme(&self.buffer, self.caret);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                let caret = movement::vertical_move(&self.buffer, self.caret, -1, TAB_WIDTH);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                let caret = movement::vertical_move(&self.buffer, self.caret, 1, TAB_WIDTH);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::Home) => {
                let caret = movement::line_start(&self.buffer, self.caret);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::End) => {
                let caret = movement::line_end(&self.buffer, self.caret);
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::PageUp) => {
                let caret = movement::vertical_move(
                    &self.buffer,
                    self.caret,
                    -self.page_lines(),
                    TAB_WIDTH,
                );
                self.move_caret(caret);
                true
            }
            Key::Named(NamedKey::PageDown) => {
                let caret =
                    movement::vertical_move(&self.buffer, self.caret, self.page_lines(), TAB_WIDTH);
                self.move_caret(caret);
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

    /// Inserta texto en el caret (teclado o commit de IME); normaliza `\r\n`.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let cleaned: Cow<'_, str> = if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        };
        self.buffer.insert(self.caret, &cleaned);
        self.caret += cleaned.chars().count();
        self.scroll_into_view();
    }

    pub fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        let start = movement::prev_grapheme(&self.buffer, self.caret);
        self.buffer.delete(start..self.caret);
        self.caret = start;
        self.scroll_into_view();
    }

    pub fn delete_forward(&mut self) {
        if self.caret >= self.buffer.len_chars() {
            return;
        }
        let end = movement::next_grapheme(&self.buffer, self.caret);
        self.buffer.delete(self.caret..end);
        self.scroll_into_view();
    }

    /// Click lógico → caret (columna visual con media celda).
    pub fn on_click(&mut self, x: f32, y: f32, advance: f32) {
        self.reset_caret_blink();
        if advance <= 0.0 {
            return;
        }
        let visual = ((x - CODE_X) / advance).floor().max(0.0) as usize;
        let line_f = ((y - PADDING_TOP + self.scroll_y) / LINE_HEIGHT).floor();
        let line = (line_f.max(0.0) as usize).min(self.buffer.line_count() - 1);
        let line_text = self.buffer.line_text(line);
        let char_col = movement::char_col_at_visual(&line_text, visual, TAB_WIDTH);
        self.caret = self.buffer.line_to_char(line) + char_col;
        self.scroll_into_view();
    }

    /// Rueda: `delta_y` en px lógicos positivos hacia arriba.
    pub fn on_wheel(&mut self, delta_y: f32) {
        self.scroll_y -= delta_y;
        self.clamp_scroll();
    }

    /// Escena de este frame: gutter + líneas visibles + caret (px lógicos).
    pub fn build_scene(&self, viewport: (f32, f32), advance_ratio: f32) -> Scene {
        let mut scene = Scene::new(srgb(BG.0, BG.1, BG.2));
        let advance = FONT_SIZE * advance_ratio;
        let line_count = self.buffer.line_count();

        let first = (((self.scroll_y - PADDING_TOP) / LINE_HEIGHT)
            .floor()
            .max(0.0) as usize)
            .min(line_count - 1);
        let last = (((self.scroll_y + viewport.1 - PADDING_TOP) / LINE_HEIGHT).ceil() as usize)
            .min(line_count - 1);

        let caret_line = self.buffer.char_to_line(self.caret);
        for line in first..=last {
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
            let col = (self.caret - self.buffer.line_to_char(caret_line)) as f32;
            scene.overlays.push(Quad::new(
                CODE_X + col * advance,
                y,
                CARET_WIDTH,
                LINE_HEIGHT - 2.0,
                CARET,
            ));
        }
        scene
    }

    fn move_caret(&mut self, caret: usize) {
        self.caret = caret;
        self.scroll_into_view();
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
