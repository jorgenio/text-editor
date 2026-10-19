//! Barra de título propia con menú embebido (M21): sin decoraciones del SO.
//!
//! La ventana se dibuja sin marco nativo (`with_decorations(false)`) y esta
//! barra ocupa la primera fila: menús a la izquierda, título de la pestaña
//! activa en el centro y los botones minimizar/maximizar/cerrar a la derecha.
//! El resto de la barra es zona de arrastre (doble clic = maximizar) y el
//! borde exterior de la ventana tiene zonas de resize para compensar la
//! pérdida del marco nativo.

use jorgenio_ui::scene::{Quad, Scene, TextSpan};
use winit::window::ResizeDirection;

use crate::settings::Settings;
use crate::theme::Theme;

/// Altura de la barra de título en px lógicos.
pub const TITLE_H: f32 = 30.0;
/// Ancho de cada botón de ventana (min/max/close).
const BUTTON_W: f32 = 46.0;
/// Altura de una fila del menú desplegado.
const ITEM_H: f32 = 26.0;
/// Padding horizontal de los items del desplegable.
const ITEM_PAD_X: f32 = 12.0;
/// Grosor del borde de resize alrededor de la ventana.
const EDGE: f32 = 6.0;

/// Qué hace un item del menú al pulsarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ejecutar el comando con ese id (los de `commands::KEYMAP`).
    Command(&'static str),
    /// Alternar una preferencia booleana.
    Toggle(&'static str),
    /// Cambiar el reparto de paneles a `columnas`×`filas` (M24).
    Layout(usize, usize),
    /// Línea separadora (no clickable).
    Separator,
}

struct MenuItem {
    label: &'static str,
    action: Action,
}

const fn item(label: &'static str, action: Action) -> MenuItem {
    MenuItem { label, action }
}

/// Menús de la barra: (raíz, items).
const MENUS: &[(&str, &[MenuItem])] = &[
    (
        "Archivo",
        &[
            item("Nuevo", Action::Command("file.new")),
            item("Abrir…", Action::Command("file.open")),
            item("Abrir rápido…", Action::Command("quickopen.show")),
            item("", Action::Separator),
            item("Guardar", Action::Command("file.save")),
            item("Guardar como…", Action::Command("file.save_as")),
            item("", Action::Separator),
            item("Cerrar pestaña", Action::Command("file.close")),
            item("Reabrir cerrada", Action::Command("file.reopen_closed")),
        ],
    ),
    (
        "Editar",
        &[
            item("Deshacer", Action::Command("edit.undo")),
            item("Rehacer", Action::Command("edit.redo")),
            item("", Action::Separator),
            item("Copiar", Action::Command("edit.copy")),
            item("Cortar", Action::Command("edit.cut")),
            item("Pegar", Action::Command("edit.paste")),
            item("", Action::Separator),
            item("Seleccionar todo", Action::Command("edit.select_all")),
            item(
                "Siguiente coincidencia",
                Action::Command("edit.select_next_occurrence"),
            ),
            item("", Action::Separator),
            item("Borrar línea", Action::Command("edit.delete_line")),
            item("Duplicar línea", Action::Command("edit.duplicate_line")),
            item("Comentar línea", Action::Command("edit.comment")),
            item("", Action::Separator),
            item("Buscar", Action::Command("find.show")),
            item("Reemplazar", Action::Command("replace.show")),
            item("Buscar en archivos", Action::Command("find_in_files.show")),
        ],
    ),
    (
        "Ver",
        &[
            item("Dividir panel", Action::Command("view.split")),
            item("Unir paneles", Action::Command("view.unsplit")),
            item("", Action::Separator),
            item("Una columna", Action::Layout(1, 1)),
            item("Dos columnas", Action::Layout(2, 1)),
            item("Dos filas", Action::Layout(1, 2)),
            item("Rejilla 2×2", Action::Layout(2, 2)),
            item("", Action::Separator),
            item("Acercar fuente", Action::Command("view.zoom_in")),
            item("Alejar fuente", Action::Command("view.zoom_out")),
            item("Fuente normal", Action::Command("view.zoom_reset")),
            item("", Action::Separator),
            item("Tema claro/oscuro", Action::Command("theme.toggle")),
        ],
    ),
    (
        "Preferencias",
        &[
            item("Abrir settings.json", Action::Command("settings.open")),
            item("", Action::Separator),
            item("Números de línea", Action::Toggle("show_line_numbers")),
            item(
                "Tabulador → espacios",
                Action::Toggle("translate_tabs_to_spaces"),
            ),
            item("Auto-match de pares", Action::Toggle("auto_match")),
        ],
    ),
];

/// Qué tocó el ratón en la barra de título.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// Ejecutar comando o alternar preferencia; el caller cierra el menú.
    Action(Action),
    /// Click en una raíz: abre/cierra su desplegado.
    Root(usize),
    /// Botones de ventana.
    Minimize,
    Maximize,
    Close,
    /// Zona libre: arrastrar la ventana (o maximizar con doble clic).
    Drag,
    /// Fuera de la barra (o en el fondo del desplegable): cerrar menú.
    Miss,
}

/// Estado de la barra: menú abierto y layout del último render.
pub struct TitleBar {
    /// Raíz del desplegado abierto (`None` = cerrado).
    open: Option<usize>,
    /// `(x, ancho)` de cada raíz, del último render.
    roots: Vec<(f32, f32)>,
    /// Fila del desplegado bajo el cursor (para el hover).
    hover: Option<usize>,
    /// `(x, y, w, h)` de cada fila del desplegado abierto (incl. sep.).
    rows: Vec<Option<(f32, f32, f32, f32)>>,
    /// Ancho del desplegado abierto.
    drop_w: f32,
    /// `(x, y, w, h)` del panel del desplegado abierto.
    drop_rect: Option<(f32, f32, f32, f32)>,
    /// `(x, w)` de los tres botones (min, max, close).
    buttons: [(f32, f32); 3],
}

impl Default for TitleBar {
    fn default() -> Self {
        Self {
            open: None,
            roots: Vec::new(),
            hover: None,
            rows: Vec::new(),
            drop_w: 0.0,
            drop_rect: None,
            buttons: [(0.0, 0.0); 3],
        }
    }
}

impl TitleBar {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
        self.hover = None;
    }

    /// Abre el primer menú (Alt, como en Windows).
    pub fn open_first(&mut self) {
        self.open = Some(0);
    }

    /// Click en la raíz `i`: abre, cierra o cambia de desplegado.
    pub fn toggle_root(&mut self, i: usize) {
        self.open = if self.open == Some(i) { None } else { Some(i) };
    }

    /// Hover del ratón sobre el desplegado (llamar desde CursorMoved).
    pub fn hover_at(&mut self, x: f32, y: f32) {
        let Some(menu) = self.open else {
            self.hover = None;
            return;
        };
        self.hover = self.rows.iter().enumerate().find_map(|(i, row)| {
            let &(rx, ry, rw, rh) = row.as_ref()?;
            (x >= rx
                && x < rx + rw
                && y >= ry
                && y < ry + rh
                && MENUS[menu].1[i].action != Action::Separator)
                .then_some(i)
        });
    }

    /// `(x, y, w, h)` del desplegado del menú abierto (o `None` si no hay
    /// menú o aún no se conoce su raíz, antes del primer render). Cálculo
    /// puro y barato: `main.rs` lo llama ANTES de `Tabs::render` para que
    /// las etiquetas de pestaña que quedan debajo no se dibujen (los texts
    /// de la escena van por encima de TODOS los quads, M26).
    pub fn drop_geometry(
        &self,
        advance: f32,
        settings: &Settings,
        layout: (usize, usize),
    ) -> Option<(f32, f32, f32, f32)> {
        let menu = self.open?;
        let (rx, _) = *self.roots.get(menu)?;
        let items = MENUS[menu].1;
        Some((
            rx,
            TITLE_H,
            drop_width(items, advance, settings, layout),
            items.len() as f32 * ITEM_H + 8.0,
        ))
    }

    /// Dibuja la barra completa (fondo, menús, título, botones y, si está
    /// abierto, el desplegado). Guarda el layout para [`TitleBar::click`].
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        scene: &mut Scene,
        viewport: (f32, f32),
        advance: f32,
        theme: &Theme,
        title: &str,
        settings: &Settings,
        layout: (usize, usize),
    ) {
        let font = (settings.font_size as f32).min(16.0);
        let text_h = font;
        let y0 = (TITLE_H - text_h) / 2.0;

        // Fondo de la barra y filete inferior.
        scene
            .top_quads
            .push(Quad::new(0.0, 0.0, viewport.0, TITLE_H, theme.bar_bg));
        scene.top_quads.push(Quad::new(
            0.0,
            TITLE_H - 1.0,
            viewport.0,
            1.0,
            theme.tab_edge,
        ));

        // Raíces del menú.
        self.roots.clear();
        let mut x = 8.0;
        for (i, (label, _)) in MENUS.iter().enumerate() {
            let w = label.chars().count() as f32 * advance + 20.0;
            if self.open == Some(i) {
                scene.top_quads.push(Quad::new(
                    x - 4.0,
                    2.0,
                    w,
                    TITLE_H - 4.0,
                    theme.tab_active_bg,
                ));
            }
            scene.top_texts.push(TextSpan::new(
                (*label).to_string(),
                x,
                y0,
                text_h,
                if self.open == Some(i) {
                    theme.fg
                } else {
                    theme.dim
                },
            ));
            self.roots.push((x - 4.0, w));
            x += w + 8.0;
        }

        // Título de la pestaña activa, centrado en el hueco disponible.
        let title_w = title.chars().count() as f32 * advance;
        let after_menus = x;
        let before_buttons = viewport.0 - 3.0 * BUTTON_W;
        let title_x = ((viewport.0 - title_w) / 2.0).max(after_menus);
        let title_x = title_x.min((before_buttons - title_w).max(after_menus));
        scene.top_texts.push(TextSpan::new(
            title.to_string(),
            title_x,
            y0,
            text_h,
            theme.dim,
        ));

        // Botones de ventana (min, max, close).
        self.buttons = [
            (viewport.0 - 3.0 * BUTTON_W, BUTTON_W),
            (viewport.0 - 2.0 * BUTTON_W, BUTTON_W),
            (viewport.0 - BUTTON_W, BUTTON_W),
        ];
        let glyphs = ["\u{2500}", "\u{25a1}", "\u{d7}"];
        for (i, &(bx, bw)) in self.buttons.iter().enumerate() {
            scene.top_texts.push(TextSpan::new(
                glyphs[i].to_string(),
                bx + (bw - text_h) / 2.0,
                y0,
                text_h,
                theme.dim,
            ));
        }

        // Desplegado del menú abierto (sus colores siguen el tema, M26).
        self.rows.clear();
        self.drop_w = 0.0;
        self.drop_rect = None;
        if let Some(menu) = self.open
            && let Some((rx, dy, drop_w, dh)) = self.drop_geometry(advance, settings, layout)
        {
            let items = MENUS[menu].1;
            self.drop_w = drop_w;
            self.drop_rect = Some((rx, dy, drop_w, dh));
            scene
                .top_quads
                .push(Quad::new(rx, dy, drop_w, dh, theme.panel_bg));
            scene
                .top_quads
                .push(Quad::new(rx, dy, drop_w, 1.0, theme.panel_edge));
            for (i, it) in items.iter().enumerate() {
                let iy = dy + 4.0 + i as f32 * ITEM_H;
                if it.action == Action::Separator {
                    scene.top_quads.push(Quad::new(
                        rx + 4.0,
                        iy + (ITEM_H - 1.0) / 2.0,
                        drop_w - 8.0,
                        1.0,
                        theme.panel_edge,
                    ));
                    self.rows.push(None);
                    continue;
                }
                if self.hover == Some(i) {
                    scene.top_quads.push(Quad::new(
                        rx + 1.0,
                        iy,
                        drop_w - 2.0,
                        ITEM_H,
                        theme.button_bg,
                    ));
                }
                let mut label = it.label.to_string();
                let mut color = theme.fg;
                match it.action {
                    Action::Toggle(field) => {
                        if toggle_on(settings, field) {
                            label = format!("\u{2713} {label}");
                        } else {
                            color = theme.dim;
                        }
                    }
                    Action::Layout(c, r) if layout == (c, r) => {
                        label = format!("\u{2713} {label}");
                    }
                    _ => {}
                }
                scene.top_texts.push(TextSpan::new(
                    label,
                    rx + ITEM_PAD_X,
                    iy + (ITEM_H - font) / 2.0,
                    font,
                    color,
                ));
                self.rows.push(Some((rx, iy, drop_w, ITEM_H)));
            }
        }
    }

    /// Click en la barra (solo llamar con `y < TITLE_H` o sobre el
    /// desplegado). `w`/`h` son el tamaño lógico de la ventana.
    pub fn click(&self, x: f32, y: f32, _w: f32, _h: f32) -> Click {
        // Botones de ventana.
        for (i, &(bx, bw)) in self.buttons.iter().enumerate() {
            if y < TITLE_H && x >= bx && x < bx + bw {
                return match i {
                    0 => Click::Minimize,
                    1 => Click::Maximize,
                    _ => Click::Close,
                };
            }
        }
        // Filas del desplegado (pueden caer por debajo de la barra).
        if let Some(menu) = self.open {
            for (i, row) in self.rows.iter().enumerate() {
                let Some((rx, ry, rw, rh)) = row.as_ref().copied() else {
                    continue;
                };
                if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                    return Click::Action(MENUS[menu].1[i].action);
                }
            }
            // Dentro del panel pero sin fila (margen): cerrar.
            if let Some((dx, dy, dw, dh)) = self.drop_rect
                && x >= dx
                && x < dx + dw
                && y >= dy
                && y < dy + dh
            {
                return Click::Miss;
            }
        }
        // Raíces del menú.
        if y < TITLE_H {
            for (i, &(rx, rw)) in self.roots.iter().enumerate() {
                if x >= rx && x < rx + rw {
                    return Click::Root(i);
                }
            }
            return Click::Drag;
        }
        Click::Miss
    }
}

/// ¿La preferencia booleana `field` está activa? (Para los checks ✓.)
fn toggle_on(settings: &Settings, field: &str) -> bool {
    match field {
        "show_line_numbers" => settings.show_line_numbers,
        "translate_tabs_to_spaces" => settings.translate_tabs_to_spaces,
        "auto_match" => settings.auto_match,
        _ => false,
    }
}

/// ¿El item debe mostrarse marcado con ✓? (Toggle encendido o el layout de
/// paneles actual.)
fn item_checked(action: Action, settings: &Settings, layout: (usize, usize)) -> bool {
    match action {
        Action::Toggle(field) => toggle_on(settings, field),
        Action::Layout(c, r) => layout == (c, r),
        _ => false,
    }
}

/// Ancho del desplegado: la etiqueta más larga (con "✓ " cuando el item
/// aparece marcado). Compartido por `render` y [`TitleBar::drop_geometry`],
/// que es quien lo llama ANTES de dibujar las pestañas.
fn drop_width(
    items: &[MenuItem],
    advance: f32,
    settings: &Settings,
    layout: (usize, usize),
) -> f32 {
    let mut drop_w: f32 = 0.0;
    for it in items {
        if it.action == Action::Separator {
            continue;
        }
        let mut label = it.label.to_string();
        if item_checked(it.action, settings, layout) {
            label = format!("\u{2713} {label}");
        }
        drop_w = drop_w.max(label.chars().count() as f32 * advance + 2.0 * ITEM_PAD_X);
    }
    drop_w
}

/// ¿`(x, y)` cae en el borde exterior de una ventana `w`×`h` (para el
/// resize sin decoraciones nativas)? Esquinas > lados.
pub fn resize_edge(x: f32, y: f32, w: f32, h: f32) -> Option<ResizeDirection> {
    let left = x < EDGE;
    let right = x >= w - EDGE;
    let top = y < EDGE;
    let bottom = y >= h - EDGE;
    match (top, bottom, left, right) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (true, _, _, true) => Some(ResizeDirection::NorthEast),
        (_, true, true, _) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::North),
        (_, true, _, _) => Some(ResizeDirection::South),
        (_, _, true, _) => Some(ResizeDirection::West),
        (_, _, _, true) => Some(ResizeDirection::East),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        Scene::new(wgpu::Color::BLACK)
    }

    fn bar() -> TitleBar {
        TitleBar::default()
    }

    fn render_into(b: &mut TitleBar, s: &mut Scene, settings: &Settings) {
        b.render(
            s,
            (800.0, 600.0),
            8.0,
            &Theme::dark(),
            "foo.rs",
            settings,
            (1, 1),
        );
    }

    fn render_layout(b: &mut TitleBar, s: &mut Scene, settings: &Settings, layout: (usize, usize)) {
        b.render(
            s,
            (800.0, 600.0),
            8.0,
            &Theme::dark(),
            "foo.rs",
            settings,
            layout,
        );
    }

    #[test]
    fn roots_render_and_click_reports_root() {
        let mut b = bar();
        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        assert_eq!(b.roots.len(), 4, "cuatro menús raíz");
        assert!(
            s.top_texts.iter().any(|t| t.text == "Archivo"),
            "dibuja las raíces"
        );
        let (rx, rw) = b.roots[0];
        assert_eq!(b.click(rx + rw / 2.0, 10.0, 800.0, 600.0), Click::Root(0));
    }

    #[test]
    fn open_menu_click_row_runs_action_and_separator_is_miss() {
        let mut b = bar();
        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        b.open = Some(0); // Archivo
        b.hover = None;
        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        // Primera fila: "Nuevo" → file.new.
        let (_, ry, _, _) = b.rows[0].expect("fila clickeable");
        assert_eq!(
            b.click(20.0, ry + 2.0, 800.0, 600.0),
            Click::Action(Action::Command("file.new"))
        );
        // Separador (índice 3): no es acción.
        assert!(b.rows[3].is_none(), "el separador no guarda fila");
    }

    #[test]
    fn buttons_hit_their_zones_and_title_body_is_drag() {
        let mut b = bar();
        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        assert_eq!(b.click(780.0, 10.0, 800.0, 600.0), Click::Close);
        assert_eq!(b.click(740.0, 10.0, 800.0, 600.0), Click::Maximize);
        assert_eq!(b.click(700.0, 10.0, 800.0, 600.0), Click::Minimize);
        // Zona libre entre los menús y los botones: arrastre.
        assert_eq!(b.click(500.0, 10.0, 800.0, 600.0), Click::Drag);
        // Debajo de la barra: miss.
        assert_eq!(b.click(300.0, 100.0, 800.0, 600.0), Click::Miss);
    }

    #[test]
    fn toggle_items_show_check_only_when_setting_on() {
        let mut s = scene();
        let mut b = bar();
        // Por defecto solo show_line_numbers está activo; activamos auto_match.
        let settings = Settings {
            auto_match: true,
            ..Settings::default()
        };
        render_into(&mut b, &mut s, &settings);
        b.open = Some(3); // Preferencias
        let mut s = scene();
        render_into(&mut b, &mut s, &settings);
        let checks: Vec<String> = s
            .top_texts
            .iter()
            .filter(|t| t.text.starts_with('\u{2713}'))
            .map(|t| t.text.clone())
            .collect();
        assert_eq!(checks.len(), 2, "números de línea + auto_match: {checks:?}");
        assert!(
            checks.iter().any(|c| c.contains("Números")),
            "números de línea marcado"
        );
        assert!(
            checks.iter().any(|c| c.contains("Auto-match")),
            "auto_match marcado"
        );
    }

    #[test]
    fn layout_menu_marks_the_current_layout_and_click_returns_it() {
        let mut b = bar();
        let mut s = scene();
        render_layout(&mut b, &mut s, &Settings::default(), (2, 2));
        b.open = Some(2); // Ver
        let mut s = scene();
        render_layout(&mut b, &mut s, &Settings::default(), (2, 2));
        let checks: Vec<String> = s
            .top_texts
            .iter()
            .filter(|t| t.text.starts_with('\u{2713}'))
            .map(|t| t.text.clone())
            .collect();
        assert_eq!(checks.len(), 1, "solo la rejilla activa: {checks:?}");
        assert!(checks[0].contains("Rejilla"), "marca la rejilla 2×2");

        // "Rejilla 2×2" es la fila 6 del menú Ver y devuelve su layout.
        let (rx, ry, _, _) = b.rows[6].expect("fila de Rejilla");
        assert_eq!(
            b.click(rx + 5.0, ry + 2.0, 800.0, 600.0),
            Click::Action(Action::Layout(2, 2))
        );
    }

    #[test]
    fn drop_geometry_matches_the_rect_render_draws() {
        let mut b = bar();
        // Sin render previo no se conocen las raíces: sin rect (y sin panics).
        b.open = Some(0);
        assert_eq!(
            b.drop_geometry(8.0, &Settings::default(), (1, 1)),
            None,
            "raíces desconocidas"
        );
        b.close();

        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        assert_eq!(
            b.drop_geometry(8.0, &Settings::default(), (1, 1)),
            None,
            "menú cerrado"
        );

        b.open = Some(0); // Archivo
        let rect = b
            .drop_geometry(8.0, &Settings::default(), (1, 1))
            .expect("rect del menú abierto");
        let mut s = scene();
        render_into(&mut b, &mut s, &Settings::default());
        assert_eq!(b.drop_rect, Some(rect), "el helper == el rect dibujado");
        assert!(
            s.top_quads
                .iter()
                .any(|q| (q.x, q.y, q.w, q.h) == rect && q.color == Theme::dark().panel_bg),
            "el fondo del desplegado ocupa ese rect con el color del tema"
        );
    }

    #[test]
    fn resize_edge_detects_corners_edges_and_center() {
        assert_eq!(
            resize_edge(2.0, 2.0, 800.0, 600.0),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(
            resize_edge(798.0, 598.0, 800.0, 600.0),
            Some(ResizeDirection::SouthEast)
        );
        assert_eq!(
            resize_edge(400.0, 2.0, 800.0, 600.0),
            Some(ResizeDirection::North)
        );
        assert_eq!(resize_edge(400.0, 300.0, 800.0, 600.0), None);
    }

    #[test]
    fn open_first_and_close() {
        let mut b = bar();
        assert!(!b.is_open());
        b.open_first();
        assert!(b.is_open());
        b.close();
        assert!(!b.is_open());
    }
}
