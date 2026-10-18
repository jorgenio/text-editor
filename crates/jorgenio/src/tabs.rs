//! Pestañas y paneles (M9/M10): hasta DOS grupos (columnas) de pestañas,
//! conmutación con Ctrl+Tab dentro del grupo activo y cierre con Ctrl+W.
//! Cada grupo dibuja su propia barra sobre su franja horizontal y el editor
//! activo reserva la barra con [`Editor::set_top_inset`] y su columna con
//! [`Editor::set_left_inset`], para que ni el texto ni los clicks queden
//! debajo ni encima de otro panel.

use jorgenio_ui::scene::{Quad, Scene, TextSpan};

use crate::editor::{Editor, FONT_SIZE};
use crate::theme::Theme;

/// Alto de la barra de pestañas (px lógicos). El área de cada editor es la
/// ventana menos este valor.
pub const TAB_BAR_H: f32 = 32.0;
/// Ancho mínimo/máximo de una pestaña (px lógicos).
const TAB_MIN_W: f32 = 96.0;
const TAB_MAX_W: f32 = 220.0;
/// Margen horizontal del texto dentro de la pestaña.
const TAB_PAD: f32 = 14.0;
/// Ancho mínimo de cada panel al arrastrar el divisor (px lógicos).
const MIN_PANEL_W: f32 = 80.0;
/// Margen de agarre del divisor alrededor de su eje (px lógicos).
const DIVIDER_GRAB: f32 = 4.0;

/// Un grupo (columna del split) con sus pestañas y su activa.
struct Group {
    list: Vec<Editor>,
    active: usize,
}

/// Franja horizontal de un grupo en el último render (para [`Tabs::click`]).
struct GroupLayout {
    x: f32,
    w: f32,
    /// (x, ancho) de cada pestaña dibujada del grupo.
    tabs: Vec<(f32, f32)>,
}

pub struct Tabs {
    groups: Vec<Group>,
    active_group: usize,
    layout: Vec<GroupLayout>,
    /// Tema de la barra y de todos los editores (lo fija `apply_prefs`).
    theme: Theme,
    /// Tamaño de la etiqueta de pestaña en px (lo fija `apply_prefs`).
    font_size: f32,
    /// Fracción del ancho de ventana que ocupa el grupo izquierdo (0..1).
    /// Solo importa con 2 grupos; se recuerda entre splits (lo mueve el
    /// divisor arrastrable). Invariante: 0 < split_ratio < 1.
    split_ratio: f32,
    /// Rutas de las últimas pestañas cerradas (la más reciente al final),
    /// para Ctrl+Shift+T. Solo paths: el contenido no se conserva.
    closed: Vec<std::path::PathBuf>,
    /// Desplazamiento vertical de las barras de pestañas (barra de título
    /// propia en M21; 0 antes de pintarla).
    top: f32,
}

/// Tope de pestañas cerradas que se recuerdan (como Sublime).
const CLOSED_MAX: usize = 10;

impl Tabs {
    /// Un solo grupo con una pestaña.
    pub fn new(first: Editor) -> Self {
        Self {
            groups: vec![Group {
                list: vec![first],
                active: 0,
            }],
            active_group: 0,
            layout: Vec::new(),
            theme: Theme::dark(),
            font_size: FONT_SIZE,
            split_ratio: 0.5,
            closed: Vec::new(),
            top: 0.0,
        }
    }

    /// Pestañas desde editores ya construidos (CLI + hot exit; M23). La
    /// activa es la primera; sin pestañas, un editor en blanco.
    pub fn from_editors(list: Vec<Editor>) -> Self {
        if list.is_empty() {
            return Self::new(Editor::default());
        }
        Self {
            groups: vec![Group { list, active: 0 }],
            active_group: 0,
            layout: Vec::new(),
            theme: Theme::dark(),
            font_size: FONT_SIZE,
            split_ratio: 0.5,
            closed: Vec::new(),
            top: 0.0,
        }
    }

    /// Todos los editores abiertos, en orden de grupo y de pestaña
    /// (para el hot exit de la sesión).
    pub fn iter(&self) -> impl Iterator<Item = &Editor> {
        self.groups.iter().flat_map(|group| group.list.iter())
    }

    /// Editor activo del grupo activo (al que van teclado, IME y comandos).
    pub fn active(&self) -> &Editor {
        let group = &self.groups[self.active_group];
        &group.list[group.active]
    }

    pub fn active_mut(&mut self) -> &mut Editor {
        let group = &mut self.groups[self.active_group];
        &mut group.list[group.active]
    }

    /// Editor activo del grupo `index` (para dibujar un split o la rueda).
    pub fn group_active_mut(&mut self, index: usize) -> &mut Editor {
        let group = &mut self.groups[index];
        &mut group.list[group.active]
    }

    /// Índice de la pestaña activa dentro de su grupo.
    pub fn active_index(&self) -> usize {
        self.groups[self.active_group].active
    }

    pub fn active_group_index(&self) -> usize {
        self.active_group
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Pestañas del grupo activo.
    pub fn len(&self) -> usize {
        self.groups[self.active_group].list.len()
    }

    /// Añade una pestaña al grupo activo y la activa; devuelve su índice.
    pub fn add(&mut self, editor: Editor) -> usize {
        let group = &mut self.groups[self.active_group];
        group.list.push(editor);
        group.active = group.list.len() - 1;
        group.active
    }

    /// Activa la pestaña `index` del grupo activo (ignora fuera de rango).
    pub fn set_active(&mut self, index: usize) {
        let group = &mut self.groups[self.active_group];
        if index < group.list.len() {
            group.active = index;
        }
    }

    /// Siguiente/anterior pestaña del grupo activo, con vuelta al extremo.
    pub fn cycle(&mut self, backwards: bool) {
        let group = &mut self.groups[self.active_group];
        let len = group.list.len();
        group.active = if backwards {
            (group.active + len - 1) % len
        } else {
            (group.active + 1) % len
        };
    }

    /// Cambia el foco de grupo sin tocar la pestaña activa de cada uno
    /// (índice fuera de rango → no hace nada).
    pub fn set_active_group(&mut self, group: usize) {
        if group < self.groups.len() {
            self.active_group = group;
        }
    }

    /// Divide en dos columnas; el nuevo grupo empieza con un archivo en
    /// blanco y toma el foco (máximo 2 paneles). El flag de `focused` de la
    /// ventana se transfiere: el grupo que pierde el foco se apaga.
    pub fn split(&mut self) {
        if self.groups.len() >= 2 {
            return;
        }
        let focused = self.active().focused();
        self.active_mut().set_focused(false);
        self.groups.push(Group {
            list: vec![Editor::default()],
            active: 0,
        });
        self.active_group = self.groups.len() - 1;
        self.active_mut().set_focused(focused);
    }

    /// Quita el split: las pestañas del grupo derecho se mueven al grupo
    /// izquierdo (nunca se pierde trabajo) y el foco sigue en el archivo que
    /// estaba activo.
    pub fn unsplit(&mut self) {
        if self.groups.len() < 2 {
            return;
        }
        let focused = self.active().focused();
        let right = self.groups.remove(1);
        if self.active_group == 1 {
            let new_active = self.groups[0].list.len() + right.active;
            self.groups[0].list.extend(right.list);
            self.groups[0].active = new_active;
        } else {
            self.groups[0].list.extend(right.list);
        }
        self.active_group = 0;
        self.active_mut().set_focused(focused);
    }

    /// Desplaza verticalmente las barras de pestañas (barra de título M21).
    pub fn set_top(&mut self, top: f32) {
        self.top = top;
    }

    /// Fija insets y viewport del editor activo del grupo `index` para este
    /// frame (`x` = origen horizontal de la columna).
    pub fn set_group_geometry(&mut self, index: usize, x: f32, w: f32, editor_h: f32) {
        let top = self.top + TAB_BAR_H;
        let editor = self.group_active_mut(index);
        editor.set_left_inset(x);
        editor.set_top_inset(top);
        editor.set_viewport(w, editor_h);
    }

    /// `(x, ancho)` de cada grupo para un `width` de ventana: un solo
    /// grupo ocupa todo; con split, el reparto sale de `split_ratio`.
    pub fn group_geometry(&self, width: f32) -> Vec<(f32, f32)> {
        if self.groups.len() < 2 {
            return vec![(0.0, width)];
        }
        let left = width * self.split_ratio;
        vec![(0.0, left), (left, width - left)]
    }

    /// Índice del grupo bajo la coordenada `x` (para clicks y rueda).
    pub fn group_at_x(&self, x: f32, width: f32) -> usize {
        match self
            .group_geometry(width)
            .iter()
            .position(|&(gx, gw)| x < gx + gw)
        {
            Some(group) => group,
            None => self.groups.len() - 1,
        }
    }

    /// ¿`x` cae sobre el divisor entre paneles (a cualquier altura)?
    pub fn over_divider(&self, x: f32, width: f32) -> bool {
        self.groups.len() >= 2 && (x - width * self.split_ratio).abs() <= DIVIDER_GRAB
    }

    /// Mueve el divisor `delta` px, recortando para que ningún panel baje
    /// de [`MIN_PANEL_W`]. La proporción queda guardada en `split_ratio`.
    pub fn drag_divider(&mut self, delta: f32, width: f32) {
        let min = (MIN_PANEL_W / width).clamp(0.05, 0.45);
        self.split_ratio = (self.split_ratio + delta / width).clamp(min, 1.0 - min);
    }

    /// Aplica preferencias a TODOS los editores abiertos (todos los grupos);
    /// el cambio de tab/gutter es solo geométrico, se ve en el siguiente frame.
    /// El tema (`"theme"` de settings) alcanza también a la barra y al split.
    pub fn apply_prefs(&mut self, prefs: &crate::settings::Settings) {
        let theme = Theme::from_name(&prefs.theme);
        self.theme = theme;
        self.font_size = prefs.font_size as f32;
        for group in &mut self.groups {
            for editor in &mut group.list {
                editor.set_tab_width(prefs.tab_width);
                editor.set_show_line_numbers(prefs.show_line_numbers);
                editor.set_font_size(prefs.font_size);
                editor.set_translate_tabs_to_spaces(prefs.translate_tabs_to_spaces);
                editor.set_auto_match(prefs.auto_match);
                editor.set_theme(theme);
            }
        }
    }

    /// ¿Ya está abierto `path` en algún grupo? Devuelve (grupo, pestaña) para
    /// enfocarlo en vez de duplicar el buffer (Ctrl+, y "Abrir archivo").
    pub fn find_path(&self, path: &std::path::Path) -> Option<(usize, usize)> {
        self.groups.iter().enumerate().find_map(|(gi, group)| {
            group
                .list
                .iter()
                .position(|editor| editor.path() == Some(path))
                .map(|tab| (gi, tab))
        })
    }

    /// Cierra la pestaña `index` (la activa si es `None`) del grupo activo y
    /// reajusta la activa. Si era la ÚLTIMA de su grupo y hay más de un
    /// grupo, se cierra el grupo entero. La última pestaña del último grupo
    /// no se cierra: siempre debe quedar dónde escribir. `true` = cerró algo.
    pub fn close(&mut self, index: Option<usize>) -> bool {
        let gi = self.active_group;
        let index = index.unwrap_or(self.groups[gi].active);
        if index >= self.groups[gi].list.len() {
            return false;
        }
        let closed_path = self.groups[gi].list[index].path().map(|p| p.to_path_buf());
        if self.groups[gi].list.len() == 1 {
            if self.groups.len() == 1 {
                return false;
            }
            self.groups.remove(gi);
            if self.active_group >= self.groups.len() {
                self.active_group = self.groups.len() - 1;
            }
            self.remember_closed(closed_path);
            return true;
        }
        self.groups[gi].list.remove(index);
        let group = &mut self.groups[gi];
        if group.active >= group.list.len() {
            group.active = group.list.len() - 1;
        } else if index < group.active {
            group.active -= 1;
        }
        self.remember_closed(closed_path);
        true
    }

    /// Apila la ruta de una pestaña cerrada (sin duplicar la última).
    fn remember_closed(&mut self, path: Option<std::path::PathBuf>) {
        let Some(path) = path else { return };
        if self.closed.last() == Some(&path) {
            return;
        }
        self.closed.push(path);
        if self.closed.len() > CLOSED_MAX {
            self.closed.remove(0);
        }
    }

    /// La última pestaña cerrada (Ctrl+Shift+T); la saca de la pila.
    pub fn take_closed(&mut self) -> Option<std::path::PathBuf> {
        self.closed.pop()
    }

    /// Barras de pestañas (una por grupo) en la capa superior, más la
    /// separación vertical entre paneles; guarda el layout para
    /// [`Tabs::click`]. Las que no caben no se dibujan ni se clican.
    pub fn render(&mut self, scene: &mut Scene, viewport: (f32, f32), advance: f32) {
        let group_count = self.groups.len();
        let gw = viewport.0 / group_count as f32;
        self.layout.clear();

        for (gi, group) in self.groups.iter().enumerate() {
            let gx = gi as f32 * gw;
            let bar_right = gx + gw;
            let top = self.top;
            scene
                .top_quads
                .push(Quad::new(gx, top, gw, TAB_BAR_H, self.theme.bar_bg));
            scene.top_quads.push(Quad::new(
                gx,
                top + TAB_BAR_H - 1.0,
                gw,
                1.0,
                self.theme.tab_edge,
            ));
            // Separación vertical con el panel vecino (altura entera).
            if gi > 0 {
                scene.top_quads.push(Quad::new(
                    gx - 1.0,
                    0.0,
                    1.0,
                    viewport.1,
                    self.theme.tab_edge,
                ));
            }

            let mut gl = GroupLayout {
                x: gx,
                w: gw,
                tabs: Vec::new(),
            };
            let mut x = gx;
            for (index, tab) in group.list.iter().enumerate() {
                if x >= bar_right {
                    break;
                }
                let mut label = tab.tab_label();
                if tab.dirty() {
                    label = format!("\u{25cf} {label}");
                }
                let w = (label.chars().count() as f32 * advance + 2.0 * TAB_PAD)
                    .clamp(TAB_MIN_W, TAB_MAX_W)
                    .min(bar_right - x);
                let is_active = index == group.active;
                scene.top_quads.push(Quad::new(
                    x,
                    top,
                    w,
                    TAB_BAR_H,
                    if is_active {
                        self.theme.tab_active_bg
                    } else {
                        self.theme.tab_bg
                    },
                ));
                // Sin borde inferior en la activa: se funde con su editor.
                if !is_active {
                    scene.top_quads.push(Quad::new(
                        x,
                        top + TAB_BAR_H - 1.0,
                        w,
                        1.0,
                        self.theme.tab_edge,
                    ));
                }
                if index > 0 {
                    scene.top_quads.push(Quad::new(
                        x,
                        top + 4.0,
                        1.0,
                        TAB_BAR_H - 8.0,
                        self.theme.tab_edge,
                    ));
                }

                // Etiqueta: se trunca por la cola si no cabe ("muy_lar…").
                // La etiqueta se limita a 24 px para no salirse de la barra
                // de 32 px; el advance se reescala para truncar con exactitud.
                let font = self.font_size.min(24.0);
                let label_advance = advance * font / self.font_size;
                let capacity = ((w - 2.0 * TAB_PAD) / label_advance).floor().max(1.0) as usize;
                if label.chars().count() > capacity {
                    let head: String = label.chars().take(capacity.saturating_sub(1)).collect();
                    label = format!("{head}\u{2026}");
                }
                scene.top_texts.push(TextSpan::new(
                    label,
                    x + TAB_PAD,
                    top + (TAB_BAR_H - font) / 2.0,
                    font,
                    if is_active {
                        self.theme.fg
                    } else {
                        self.theme.dim
                    },
                ));

                gl.tabs.push((x, w));
                x += w;
            }
            self.layout.push(gl);
        }
    }

    /// Click en alguna barra: grupo y pestaña tocadas, o `None` si no fue
    /// dentro de la barra o de una pestaña dibujada.
    pub fn click(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        if y < self.top || y >= self.top + TAB_BAR_H {
            return None;
        }
        let group = self
            .layout
            .iter()
            .position(|gl| x >= gl.x && x < gl.x + gl.w)?;
        let tab = self.layout[group]
            .tabs
            .iter()
            .position(|&(tx, w)| x >= tx && x < tx + w)?;
        Some((group, tab))
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
    fn close_remembers_paths_for_reopen() {
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.add(tab_named("b.rs"));
        assert!(tabs.close(Some(0)), "cierra a.rs");
        assert_eq!(
            tabs.take_closed().map(|p| p.to_string_lossy().into_owned()),
            Some("a.rs".to_owned()),
            "la última cerrada sale de la pila"
        );
        assert!(tabs.take_closed().is_none(), "la pila se vacía");

        // La única pestaña del único grupo no se cierra ni se recuerda.
        assert!(!tabs.close(None));
        assert!(tabs.take_closed().is_none());
    }

    /// Editor CON ruta (sin tocar disco): `tab_label` es el nombre del archivo.
    fn tab_named(name: &str) -> Editor {
        Editor::from_cli(std::path::PathBuf::from(name))
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
    fn from_editors_keeps_order_and_empty_falls_back_to_blank() {
        let blank = Tabs::from_editors(Vec::new());
        assert_eq!(blank.len(), 1);
        assert!(!blank.active().has_path());

        let tabs = Tabs::from_editors(vec![
            Editor::from_cli(std::path::PathBuf::from("no_existe_a.txt")),
            Editor::from_cli(std::path::PathBuf::from("no_existe_b.txt")),
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

        let tab0 = tabs.layout[0].tabs[0];
        let tab1 = tabs.layout[0].tabs[1];
        assert_eq!(
            tabs.click(tab0.0 + tab0.1 / 2.0, TAB_BAR_H / 2.0),
            Some((0, 0))
        );
        assert_eq!(
            tabs.click(tab1.0 + tab1.1 / 2.0, TAB_BAR_H / 2.0),
            Some((0, 1))
        );
        assert_eq!(tabs.click(0.0, TAB_BAR_H + 1.0), None, "bajo la barra");
        assert_eq!(tabs.click(799.0, 5.0), None, "pestaña que no cabe dibujada");

        // Muchas pestañas: no se dibuja ni se clica lo que no cabe.
        for i in 0..40 {
            tabs.add(tab_with(&format!("archivo_{i}")));
        }
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (300.0, 600.0), 8.0);
        assert!(tabs.layout[0].tabs.len() < 40, "solo caben unas pocas");
        assert!(tabs.layout[0].tabs.iter().all(|&(x, w)| x + w <= 300.0));
    }

    #[test]
    fn split_creates_second_group_and_unsplit_preserves_tabs() {
        let mut tabs = Tabs::new(tab_with("uno"));
        tabs.add(tab_with("dos"));
        tabs.split();
        assert_eq!(tabs.group_count(), 2);
        assert_eq!(tabs.active_group_index(), 1, "el foco pasa al nuevo");
        assert_eq!(tabs.active().tab_label(), "sin título", "en blanco");
        tabs.add(tab_named("tres.rs"));

        tabs.unsplit();
        assert_eq!(tabs.group_count(), 1);
        assert_eq!(tabs.len(), 4, "las pestañas del grupo derecho se conservan");
        assert_eq!(tabs.active().tab_label(), "tres.rs", "sigue el foco");
        // unsplit sin split no rompe nada.
        tabs.unsplit();
        assert_eq!(tabs.group_count(), 1);
    }

    #[test]
    fn closing_last_tab_of_a_group_closes_the_group() {
        let mut tabs = Tabs::new(tab_named("uno.rs"));
        tabs.split();
        assert_eq!(tabs.group_count(), 2);
        assert!(tabs.close(None), "cierra la única pestaña del grupo");
        assert_eq!(tabs.group_count(), 1, "el grupo derecho desaparece");
        assert_eq!(tabs.active_group_index(), 0);
        assert_eq!(tabs.active().tab_label(), "uno.rs");
    }

    #[test]
    fn split_geometry_and_click_cover_both_panels() {
        let mut tabs = Tabs::new(tab_named("uno.rs"));
        tabs.split();

        tabs.set_group_geometry(1, 500.0, 500.0, 568.0);
        assert_eq!(tabs.group_active_mut(1).left_inset(), 500.0);

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (1000.0, 600.0), 8.0);
        assert_eq!(tabs.layout.len(), 2, "una franja por grupo");
        assert_eq!(tabs.layout[0].x, 0.0);
        assert_eq!(tabs.layout[1].x, 500.0, "la segunda columna empieza a 500");
        assert_eq!(scene.top_texts.len(), 2, "una pestaña por grupo");
        assert!(
            scene.top_quads.iter().any(|q| q.h == 600.0 && q.w == 1.0),
            "separador vertical a altura completa"
        );

        // El click resuelve (grupo, pestaña) por la x dentro de cada barra;
        // el hueco vacío de la barra no es clicable.
        assert_eq!(tabs.click(50.0, TAB_BAR_H / 2.0), Some((0, 0)));
        assert_eq!(tabs.click(550.0, TAB_BAR_H / 2.0), Some((1, 0)));
        assert_eq!(tabs.click(700.0, TAB_BAR_H / 2.0), None, "hueco vacío");
        assert_eq!(tabs.click(700.0, TAB_BAR_H + 1.0), None, "bajo la barra");

        // El cambio cruzado de grupo lo resuelve el shell (focus_tab) con
        // set_active_group + set_active.
        tabs.set_active_group(0);
        tabs.set_active(0);
        assert_eq!(tabs.active_group_index(), 0);
        assert_eq!(tabs.active().tab_label(), "uno.rs");
    }

    #[test]
    fn apply_prefs_reaches_every_group_and_find_path_locates_tabs() {
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.add(tab_named("b.rs"));
        tabs.split();
        tabs.add(tab_named("c.rs")); // en el grupo derecho

        let prefs = crate::settings::Settings {
            tab_width: 8,
            show_line_numbers: false,
            translate_tabs_to_spaces: true,
            auto_match: false,
            ..crate::settings::Settings::default()
        };
        tabs.apply_prefs(&prefs);

        for gi in 0..tabs.group_count() {
            let editor = tabs.group_active_mut(gi);
            assert_eq!(editor.tab_width(), 8, "grupo {gi} actualizado");
            assert!(!editor.show_line_numbers(), "grupo {gi} sin gutter");
            assert!(editor.translate_tabs_to_spaces(), "grupo {gi} con espacios");
            assert!(!editor.auto_match(), "grupo {gi} sin auto_match");
        }

        assert_eq!(
            tabs.find_path(std::path::Path::new("b.rs")),
            Some((0, 1)),
            "la pestaña del grupo izquierdo"
        );
        assert_eq!(
            tabs.find_path(std::path::Path::new("c.rs")),
            Some((1, 1)),
            "la pestaña del grupo derecho"
        );
        assert_eq!(tabs.find_path(std::path::Path::new("no.rs")), None);
    }

    #[test]
    fn apply_prefs_applies_the_theme_to_bar_and_editors() {
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.add(tab_named("b.rs"));
        tabs.split();
        assert_eq!(tabs.theme, Theme::dark(), "arranca oscuro");

        let prefs = crate::settings::Settings {
            theme: "light".to_owned(),
            ..crate::settings::Settings::default()
        };
        tabs.apply_prefs(&prefs);
        assert_eq!(tabs.theme, Theme::light(), "la barra sigue el tema");
        assert_eq!(tabs.active().theme(), Theme::light());
        assert_eq!(
            tabs.group_active_mut(1).theme(),
            Theme::light(),
            "todos los grupos, no solo el activo"
        );

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0);
        assert_eq!(scene.top_quads[0].color, Theme::light().bar_bg);
    }

    #[test]
    fn apply_prefs_sets_the_font_size_on_bar_and_editors() {
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.add(tab_named("b.rs"));
        tabs.split();
        assert_eq!(tabs.font_size, FONT_SIZE, "arranca con el default");

        let prefs = crate::settings::Settings {
            font_size: 24,
            ..crate::settings::Settings::default()
        };
        tabs.apply_prefs(&prefs);
        assert_eq!(tabs.font_size, 24.0, "la etiqueta de la barra");
        assert_eq!(tabs.active().font_size(), 24.0, "editor activo");
        assert_eq!(
            tabs.group_active_mut(1).font_size(),
            24.0,
            "todos los grupos, no solo el activo"
        );

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 12.0);
        let label = scene.top_texts.first().expect("etiqueta dibujada");
        assert_eq!(label.size, 24.0, "la etiqueta usa el tamaño pedido");
        assert_eq!(label.y, (TAB_BAR_H - 24.0) / 2.0, "centrada en la barra");

        // Por encima de 24 px la etiqueta se limita para no salirse de
        // la barra de 32 px.
        let big = crate::settings::Settings {
            font_size: 40,
            ..crate::settings::Settings::default()
        };
        tabs.apply_prefs(&big);
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 16.0);
        let label = scene.top_texts.first().expect("etiqueta dibujada");
        assert_eq!(label.size, 24.0, "tope por la altura de la barra");
    }

    #[test]
    fn group_geometry_splits_by_ratio_and_divider_follows_it() {
        let mut tabs = Tabs::new(Editor::default());
        assert_eq!(
            tabs.group_geometry(1000.0),
            vec![(0.0, 1000.0)],
            "un solo grupo ocupa todo el ancho"
        );
        assert_eq!(tabs.group_at_x(500.0, 1000.0), 0);
        assert!(
            !tabs.over_divider(500.0, 1000.0),
            "sin split no hay divisor"
        );

        tabs.split();
        assert_eq!(
            tabs.group_geometry(1000.0),
            vec![(0.0, 500.0), (500.0, 500.0)],
            "reparto por defecto 50/50"
        );
        assert!(tabs.over_divider(500.0, 1000.0), "sobre el eje");
        assert!(tabs.over_divider(503.0, 1000.0), "el agarre es de pocos px");
        assert!(!tabs.over_divider(510.0, 1000.0), "fuera del agarre");
        assert_eq!(tabs.group_at_x(501.0, 1000.0), 1, "click a la derecha");

        // 0.5 + 125/1000 = 0.625 (exacto en binario): 625/375.
        tabs.drag_divider(125.0, 1000.0);
        assert_eq!(
            tabs.group_geometry(1000.0),
            vec![(0.0, 625.0), (625.0, 375.0)],
            "el divisor sigue al arrastre"
        );
        assert!(tabs.over_divider(625.0, 1000.0), "el divisor se mueve");

        tabs.drag_divider(-10_000.0, 1000.0);
        let left = tabs.group_geometry(1000.0)[0].1;
        assert!(
            (left - 80.0).abs() < 0.01,
            "ancho mínimo izquierdo, got {left}"
        );
        tabs.drag_divider(10_000.0, 1000.0);
        let right = tabs.group_geometry(1000.0)[1].1;
        assert!(
            (right - 80.0).abs() < 0.01,
            "ancho mínimo derecho, got {right}"
        );
    }

    #[test]
    fn unsplit_remembers_the_ratio_for_the_next_split() {
        let mut tabs = Tabs::new(Editor::default());
        tabs.split();
        tabs.drag_divider(100.0, 1000.0); // 0.5 + 0.1 = 0.6

        tabs.unsplit();
        assert_eq!(
            tabs.group_geometry(1200.0),
            vec![(0.0, 1200.0)],
            "sin split vuelve a un panel"
        );
        assert!(!tabs.over_divider(600.0, 1200.0));

        tabs.split();
        let left = tabs.group_geometry(1000.0)[0].1;
        assert!(
            (left - 600.0).abs() < 0.01,
            "recuerda el reparto arrastrado"
        );
    }
}
