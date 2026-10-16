//! Barra de pestañas y conjunto de editores (M9): una pestaña por archivo,
//! conmutación con Ctrl+Tab y cierre con Ctrl+W. La barra se dibuja en la
//! capa superior y el editor activo reserva su alto con
//! [`Editor::set_top_inset`] para que ni el texto ni los clicks queden debajo.

use jorgenio_ui::scene::{Quad, Scene, TextSpan};

use crate::editor::{Editor, FONT_SIZE};

/// Alto de la barra de pestañas (px lógicos). El área del editor es la
/// ventana menos este valor.
pub const TAB_BAR_H: f32 = 32.0;
/// Ancho mínimo/máximo de una pestaña (px lógicos).
const TAB_MIN_W: f32 = 96.0;
const TAB_MAX_W: f32 = 220.0;
/// Margen horizontal del texto dentro de la pestaña.
const TAB_PAD: f32 = 14.0;

const BAR_BG: (u8, u8, u8, u8) = (0x1A, 0x1B, 0x18, 0xFF);
const TAB_BG: (u8, u8, u8, u8) = (0x22, 0x23, 0x1E, 0xFF);
/// Pestaña activa: mismo tono que el fondo del editor (se funde con él).
const TAB_ACTIVE_BG: (u8, u8, u8, u8) = (0x27, 0x28, 0x22, 0xFF);
const TAB_EDGE: (u8, u8, u8, u8) = (0x3E, 0x3D, 0x32, 0xFF);
const FG: (u8, u8, u8) = (0xF8, 0xF8, 0xF2);
const DIM: (u8, u8, u8) = (0x75, 0x71, 0x5E);

pub struct Tabs {
    list: Vec<Editor>,
    active: usize,
    /// (x, ancho) de cada pestaña dibujada en el último render (para click).
    layout: Vec<(f32, f32)>,
}

impl Tabs {
    /// Una sola pestaña con `first`.
    pub fn new(first: Editor) -> Self {
        Self {
            list: vec![first],
            active: 0,
            layout: Vec::new(),
        }
    }

    /// Pestañas desde los paths de la CLI (ninguno → un editor en blanco);
    /// la activa es la primera.
    pub fn from_paths(paths: Vec<std::path::PathBuf>) -> Self {
        if paths.is_empty() {
            return Self::new(Editor::default());
        }
        let list = paths.into_iter().map(Editor::from_cli).collect();
        Self {
            list,
            active: 0,
            layout: Vec::new(),
        }
    }

    pub fn active(&self) -> &Editor {
        &self.list[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Editor {
        &mut self.list[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Añade una pestaña al final y la activa; devuelve su índice.
    pub fn add(&mut self, editor: Editor) -> usize {
        self.list.push(editor);
        self.active = self.list.len() - 1;
        self.active
    }

    /// Activa la pestaña `index` (ignora índices fuera de rango).
    pub fn set_active(&mut self, index: usize) {
        if index < self.list.len() {
            self.active = index;
        }
    }

    /// Siguiente/anterior pestaña, con vuelta al otro extremo.
    pub fn cycle(&mut self, backwards: bool) {
        let len = self.list.len();
        self.active = if backwards {
            (self.active + len - 1) % len
        } else {
            (self.active + 1) % len
        };
    }

    /// Cierra la pestaña `index` (la activa si es `None`) y reajusta la
    /// activa. La ÚLTIMA pestaña no se cierra: siempre debe quedar dónde
    /// escribir. `true` = se cerró.
    pub fn close(&mut self, index: Option<usize>) -> bool {
        let index = index.unwrap_or(self.active);
        if index >= self.list.len() || self.list.len() == 1 {
            return false;
        }
        self.list.remove(index);
        if self.active >= self.list.len() {
            self.active = self.list.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        true
    }

    /// Barra de pestañas en la capa superior; guarda el layout para
    /// [`Tabs::click`]. Las que no caben no se dibujan ni se clican.
    pub fn render(&mut self, scene: &mut Scene, viewport: (f32, f32), advance: f32) {
        scene
            .top_quads
            .push(Quad::new(0.0, 0.0, viewport.0, TAB_BAR_H, BAR_BG));
        scene
            .top_quads
            .push(Quad::new(0.0, TAB_BAR_H - 1.0, viewport.0, 1.0, TAB_EDGE));

        self.layout.clear();
        let mut x = 0.0;
        for (index, tab) in self.list.iter().enumerate() {
            if x >= viewport.0 {
                break;
            }
            let dirty = tab.dirty();
            let mut label = tab.tab_label();
            if dirty {
                label = format!("\u{25cf} {label}");
            }
            let w = (label.chars().count() as f32 * advance + 2.0 * TAB_PAD)
                .clamp(TAB_MIN_W, TAB_MAX_W)
                .min(viewport.0 - x);
            let is_active = index == self.active;
            scene.top_quads.push(Quad::new(
                x,
                0.0,
                w,
                TAB_BAR_H,
                if is_active { TAB_ACTIVE_BG } else { TAB_BG },
            ));
            // Sin borde inferior en la activa: se funde con el editor.
            if !is_active {
                scene
                    .top_quads
                    .push(Quad::new(x, TAB_BAR_H - 1.0, w, 1.0, TAB_EDGE));
            }
            if index > 0 {
                scene
                    .top_quads
                    .push(Quad::new(x, 4.0, 1.0, TAB_BAR_H - 8.0, TAB_EDGE));
            }

            // Etiqueta: se trunca por la cola si no cabe ("muy_lar…").
            let capacity = ((w - 2.0 * TAB_PAD) / advance).floor().max(1.0) as usize;
            if label.chars().count() > capacity {
                let head: String = label.chars().take(capacity.saturating_sub(1)).collect();
                label = format!("{head}\u{2026}");
            }
            scene.top_texts.push(TextSpan::new(
                label,
                x + TAB_PAD,
                (TAB_BAR_H - FONT_SIZE) / 2.0,
                FONT_SIZE,
                if is_active { FG } else { DIM },
            ));

            self.layout.push((x, w));
            x += w;
        }
    }

    /// Click en la barra: índice de la pestaña tocada, o `None` si no fue
    /// dentro de la barra o de una pestaña dibujada.
    pub fn click(&self, x: f32, y: f32) -> Option<usize> {
        if y < 0.0 || y >= TAB_BAR_H {
            return None;
        }
        self.layout
            .iter()
            .position(|&(tx, w)| x >= tx && x < tx + w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with(label: &str) -> Editor {
        let mut editor = Editor::default();
        editor.insert_text(label);
        editor
    }

    #[test]
    fn new_add_cycle_and_close_adjust_active() {
        let mut tabs = Tabs::new(tab_with("uno"));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);

        assert_eq!(tabs.add(tab_with("dos")), 1, "add activa la nueva");
        assert_eq!(tabs.add(tab_with("tres")), 2);

        tabs.cycle(false);
        assert_eq!(tabs.active_index(), 0, "ciclo con vuelta");
        tabs.cycle(true);
        assert_eq!(tabs.active_index(), 2);
        tabs.set_active(1);
        assert_eq!(tabs.active_index(), 1);

        // Cerrar la ACTIVA: la siguiente ocupa su sitio.
        assert!(tabs.close(None));
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), 1, "tras cerrar la 1, la 2 baja a 1");
        // Cerrar una ANTERIOR a la activa: el índice se corre.
        assert!(tabs.close(Some(0)));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0, "índice corregido");

        // La última pestaña no se cierra.
        assert!(!tabs.close(None));
        assert_eq!(tabs.len(), 1);
        assert!(!tabs.close(Some(3)), "índice fuera de rango");
    }

    #[test]
    fn from_paths_opens_all_and_empty_falls_back_to_blank() {
        let blank = Tabs::from_paths(Vec::new());
        assert_eq!(blank.len(), 1);
        assert!(!blank.active().has_path());

        let tabs = Tabs::from_paths(vec![
            std::path::PathBuf::from("no_existe_a.txt"),
            std::path::PathBuf::from("no_existe_b.txt"),
        ]);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), 0, "la activa es la primera");
        assert!(tabs.active().has_path());
    }

    #[test]
    fn render_draws_each_tab_and_click_hits_it() {
        let mut tabs = Tabs::new(tab_with("uno"));
        tabs.add(tab_with("dos"));

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0);
        // Fondo + borde inferior + (bg + borde + separador + bg + separador).
        assert!(
            scene.top_quads.len() >= 6,
            "fondo + tabs: {}",
            scene.top_quads.len()
        );
        assert_eq!(scene.top_texts.len(), 2, "una etiqueta por pestaña");

        let (x0, w0) = tabs.layout[0];
        let (x1, w1) = tabs.layout[1];
        assert_eq!(tabs.click(x0 + w0 / 2.0, TAB_BAR_H / 2.0), Some(0));
        assert_eq!(tabs.click(x1 + w1 / 2.0, TAB_BAR_H / 2.0), Some(1));
        assert_eq!(tabs.click(0.0, TAB_BAR_H + 1.0), None, "bajo la barra");
        assert_eq!(tabs.click(799.0, 5.0), None, "pestaña que no cabe dibujada");

        // Muchas pestañas: no se dibuja ni se clica lo que no cabe.
        for i in 0..40 {
            tabs.add(tab_with(&format!("archivo_{i}")));
        }
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (300.0, 600.0), 8.0);
        assert!(tabs.layout.len() < 40, "solo caben unas pocas");
        assert!(tabs.layout.iter().all(|&(x, w)| x + w <= 300.0));
    }
}
