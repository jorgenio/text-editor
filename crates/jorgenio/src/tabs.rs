//! Pestañas y paneles (M9/M10/M24): rejilla de hasta 2×2 grupos de pestañas
//! (una columna, dos columnas, dos filas o rejilla), conmutación con Ctrl+Tab
//! dentro del grupo activo y cierre con Ctrl+W. Cada grupo (celda) dibuja su
//! propia barra sobre su franja y el editor activo reserva la barra con
//! [`Editor::set_top_inset`] y su celda con [`Editor::set_left_inset`], para
//! que ni el texto ni los clicks queden debajo ni encima de otro panel.

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
/// Zona de la × de cierre, SIEMPRE reservada en el ancho de cada pestaña
/// (así el hover no recoloca nada) y en la capacidad de su etiqueta.
const TAB_CLOSE_W: f32 = 16.0;
/// Ancho mínimo de cada panel al arrastrar el divisor (px lógicos).
const MIN_PANEL_W: f32 = 80.0;
/// Margen de agarre del divisor alrededor de su eje (px lógicos).
const DIVIDER_GRAB: f32 = 4.0;
/// Alto mínimo de una fila al arrastrar el divisor horizontal (barra de
/// pestañas + un editor usable).
const MIN_ROW_H: f32 = TAB_BAR_H + 30.0;

/// Reparto de la ventana entre celdas (M24). Cada variante fija el número de
/// grupos: `groups.len()` siempre igual al número de celdas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Un grupo a pantalla completa.
    Single,
    /// Dos columnas separadas por el divisor vertical (`split_ratio`).
    Columns,
    /// Dos filas separadas por el divisor horizontal (`v_ratio`).
    Rows,
    /// Rejilla 2×2: ambos divisores.
    Grid,
}

impl Layout {
    /// Número de celdas (y por tanto de grupos) del reparto.
    fn cells(self) -> usize {
        match self {
            Layout::Single => 1,
            Layout::Columns | Layout::Rows => 2,
            Layout::Grid => 4,
        }
    }

    /// `(columnas, filas)` del reparto.
    fn dims(self) -> (usize, usize) {
        match self {
            Layout::Single => (1, 1),
            Layout::Columns => (2, 1),
            Layout::Rows => (1, 2),
            Layout::Grid => (2, 2),
        }
    }
}

/// Eje de un divisor arrastrable (M24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Entre columnas (se mueve en X).
    Vertical,
    /// Entre filas (se mueve en Y).
    Horizontal,
}

/// Qué tocó el ratón dentro de una barra de pestañas (M26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    /// Cambiar a esa pestaña: `(grupo, pestaña)`.
    Select(usize, usize),
    /// Pulsar su × de cierre: `(grupo, pestaña)`.
    Close(usize, usize),
}

/// ¿El rect `(x, y, w, h)` intersecta con `occluded`?
fn covered(occluded: Option<(f32, f32, f32, f32)>, x: f32, y: f32, w: f32, h: f32) -> bool {
    occluded.is_some_and(|(ox, oy, ow, oh)| x < ox + ow && x + w > ox && y < oy + oh && y + h > oy)
}

/// Un grupo (columna del split) con sus pestañas y su activa.
struct Group {
    list: Vec<Editor>,
    active: usize,
}

/// Franja de un grupo en el último render (para [`Tabs::click`]).
struct GroupLayout {
    x: f32,
    /// Origen vertical de la barra de pestañas del grupo (0 en la fila 0).
    y: f32,
    w: f32,
    /// (x, ancho) de cada pestaña dibujada del grupo.
    tabs: Vec<(f32, f32)>,
}

pub struct Tabs {
    groups: Vec<Group>,
    active_group: usize,
    layout: Vec<GroupLayout>,
    /// Reparto de la ventana entre celdas (M24).
    panes: Layout,
    /// Tema de la barra y de todos los editores (lo fija `apply_prefs`).
    theme: Theme,
    /// Tamaño de la etiqueta de pestaña en px (lo fija `apply_prefs`).
    font_size: f32,
    /// Fracción del ancho de ventana que ocupa la columna izquierda (0..1).
    /// Solo importa con 2 columnas; se recuerda entre splits (lo mueve el
    /// divisor arrastrable). Invariante: 0 < split_ratio < 1.
    split_ratio: f32,
    /// Fracción de la altura de contenido que ocupa la fila superior (0..1).
    /// Solo importa con 2 filas; igual que `split_ratio` pero en el eje Y.
    v_ratio: f32,
    /// Rutas de las últimas pestañas cerradas (la más reciente al final),
    /// para Ctrl+Shift+T. Solo paths: el contenido no se conserva.
    closed: Vec<std::path::PathBuf>,
    /// Puntero del ratón en lógicas (hover de la barra: × y resaltado).
    pointer: (f32, f32),
    /// Rect del menú desplegado de la barra de título que tapa etiquetas:
    /// los textos van por encima de TODOS los quads, así que no se dibujan
    /// (M26; `main.rs` lo pasa con `set_occluded` antes de `render`).
    occluded: Option<(f32, f32, f32, f32)>,
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
            panes: Layout::Single,
            theme: Theme::dark(),
            font_size: FONT_SIZE,
            split_ratio: 0.5,
            v_ratio: 0.5,
            closed: Vec::new(),
            pointer: (-1.0, -1.0),
            occluded: None,
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
            panes: Layout::Single,
            theme: Theme::dark(),
            font_size: FONT_SIZE,
            split_ratio: 0.5,
            v_ratio: 0.5,
            closed: Vec::new(),
            pointer: (-1.0, -1.0),
            occluded: None,
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

    /// Número de grupos (celdas con pestañas); solo lo usan los tests — la
    /// geometría trabaja con [`Tabs::layout`] y las celdas.
    #[cfg(test)]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Reparto actual `(columnas, filas)` (para el menú Ver y su checkmark).
    pub fn layout(&self) -> (usize, usize) {
        self.panes.dims()
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

    /// Divide añadiendo una columna: una sola celda pasa a dos columnas (o
    /// dos filas a rejilla). El grupo nuevo (columna derecha) empieza en
    /// blanco y toma el foco. El flag de `focused` de la ventana se transfiere.
    pub fn split(&mut self) {
        let new = match self.panes {
            Layout::Single => Layout::Columns,
            Layout::Rows => Layout::Grid,
            // Ya hay 2 columnas: no hay columna que añadir.
            Layout::Columns | Layout::Grid => return,
        };
        let focused = self.active().focused();
        self.active_mut().set_focused(false);
        while self.groups.len() < new.cells() {
            self.groups.push(Group {
                list: vec![Editor::default()],
                active: 0,
            });
        }
        self.panes = new;
        self.active_group = self.groups.len() - 1;
        self.active_mut().set_focused(focused);
    }

    /// Quita la columna derecha (una columna pasa a celda única o rejilla a
    /// dos filas): las pestañas de esa columna se funden con las de su fila
    /// (nunca se pierde trabajo) y el foco sigue en el archivo que estaba
    /// activo.
    pub fn unsplit(&mut self) {
        let new = match self.panes {
            Layout::Columns => Layout::Single,
            Layout::Grid => Layout::Rows,
            // Sin columna derecha que quitar.
            Layout::Single | Layout::Rows => return,
        };
        let (cols_before, rows_before) = self.panes.dims();
        let focused = self.active().focused();
        let was_active = self.active_group;
        let was_row = was_active / cols_before;
        let was_right = was_active % cols_before == 1;

        // Reconstruye: funde la columna derecha de cada fila en la izquierda.
        let mut old = std::mem::take(&mut self.groups).into_iter();
        let mut kept: Vec<Group> = Vec::with_capacity(rows_before);
        let mut new_active = 0;
        for r in 0..rows_before {
            let mut left = old.next().expect("columna izquierda de la fila");
            let right = old.next().expect("columna derecha de la fila");
            let offset = left.list.len();
            left.list.extend(right.list);
            if r == was_row {
                new_active = r;
                if was_right {
                    left.active = offset + right.active;
                }
            }
            kept.push(left);
        }
        self.groups = kept;
        self.panes = new;
        self.active_group = new_active;
        self.active_mut().set_focused(focused);
    }

    /// Cambia a un reparto `columns`×`rows`, conservando las pestañas: las
    /// celdas nuevas empiezan en blanco y, si se reducen, las celdas que
    /// sobran se funden en la primera (nunca se pierde trabajo).
    pub fn set_layout(&mut self, columns: usize, rows: usize) {
        let new = match (columns.clamp(1, 2), rows.clamp(1, 2)) {
            (1, 1) => Layout::Single,
            (2, 1) => Layout::Columns,
            (1, 2) => Layout::Rows,
            _ => Layout::Grid,
        };
        if new == self.panes {
            return;
        }
        let target = new.cells();
        if target > self.groups.len() {
            while self.groups.len() < target {
                self.groups.push(Group {
                    list: vec![Editor::default()],
                    active: 0,
                });
            }
        } else if target < self.groups.len() {
            let focused = self.active().focused();
            let extra: Vec<Group> = self.groups.drain(target..).collect();
            for g in extra {
                self.groups[0].list.extend(g.list);
            }
            if self.groups[0].active >= self.groups[0].list.len() {
                self.groups[0].active = self.groups[0].list.len() - 1;
            }
            self.active_group = 0;
            self.active_mut().set_focused(focused);
        }
        self.panes = new;
        if self.active_group >= self.groups.len() {
            self.active_group = self.groups.len() - 1;
        }
    }

    /// Fija insets y viewport del editor activo del grupo `index` para este
    /// frame. `(x, y, w, h)` es la celda COMPLETA (barra de pestañas
    /// incluida): el editor queda bajo la barra, `TAB_BAR_H` más abajo.
    pub fn set_group_geometry(&mut self, index: usize, x: f32, y: f32, w: f32, h: f32) {
        let editor = self.group_active_mut(index);
        editor.set_left_inset(x);
        editor.set_top_inset(y + TAB_BAR_H);
        editor.set_viewport(w, (h - TAB_BAR_H).max(50.0));
    }

    /// Puntero del ratón para el hover de la barra (× y resaltado); lo
    /// llama `main.rs` en cada `CursorMoved` (y fuera de la ventana con
    /// `(-1, -1)`, para que la × desaparezca).
    pub fn set_pointer(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
    }

    /// Rect `(x, y, w, h)` del menú desplegado de la barra de título: sus
    /// etiquetas de pestaña NO se dibujan este frame (`main.rs` lo pasa
    /// antes de `render`, porque los textos van por encima de todo).
    pub fn set_occluded(&mut self, rect: Option<(f32, f32, f32, f32)>) {
        self.occluded = rect;
    }

    /// Celdas `(x, y, w, h)` de cada grupo para una ventana de `width` y un
    /// área de contenido `(content_top, content_h)` (barra de título arriba,
    /// barra de estado abajo). Las celdas INCLUYEN la barra de pestañas.
    pub fn group_geometry(
        &self,
        width: f32,
        content_top: f32,
        content_h: f32,
    ) -> Vec<(f32, f32, f32, f32)> {
        match self.panes {
            Layout::Single => vec![(0.0, content_top, width, content_h)],
            Layout::Columns => {
                let left = width * self.split_ratio;
                vec![
                    (0.0, content_top, left, content_h),
                    (left, content_top, width - left, content_h),
                ]
            }
            Layout::Rows => {
                let top_h = content_h * self.v_ratio;
                vec![
                    (0.0, content_top, width, top_h),
                    (0.0, content_top + top_h, width, content_h - top_h),
                ]
            }
            Layout::Grid => {
                let left = width * self.split_ratio;
                let top_h = content_h * self.v_ratio;
                vec![
                    (0.0, content_top, left, top_h),
                    (left, content_top, width - left, top_h),
                    (0.0, content_top + top_h, left, content_h - top_h),
                    (left, content_top + top_h, width - left, content_h - top_h),
                ]
            }
        }
    }

    /// Índice del grupo bajo `(x, y)` (para clicks y rueda).
    pub fn group_at(&self, x: f32, y: f32, width: f32, content_top: f32, content_h: f32) -> usize {
        self.group_geometry(width, content_top, content_h)
            .iter()
            .position(|&(cx, cy, cw, ch)| x >= cx && x < cx + cw && y >= cy && y < cy + ch)
            .unwrap_or(0)
    }

    /// ¿`(x, y)` cae sobre algún divisor? Devuelve el eje (para el icono de
    /// ratón y para saber qué razón arrastrar).
    pub fn over_divider(
        &self,
        x: f32,
        y: f32,
        width: f32,
        content_top: f32,
        content_h: f32,
    ) -> Option<Divider> {
        match self.panes {
            Layout::Single => None,
            Layout::Columns => {
                ((x - width * self.split_ratio).abs() <= DIVIDER_GRAB).then_some(Divider::Vertical)
            }
            Layout::Rows => ((y - (content_top + content_h * self.v_ratio)).abs() <= DIVIDER_GRAB)
                .then_some(Divider::Horizontal),
            Layout::Grid => {
                if (x - width * self.split_ratio).abs() <= DIVIDER_GRAB {
                    Some(Divider::Vertical)
                } else if (y - (content_top + content_h * self.v_ratio)).abs() <= DIVIDER_GRAB {
                    Some(Divider::Horizontal)
                } else {
                    None
                }
            }
        }
    }

    /// Mueve el divisor `divider` `delta` px sobre `extent` (el ancho de la
    /// ventana para el vertical, la altura de contenido para el horizontal),
    /// recortando para que ninguna celda baje de su mínimo. Las proporciones
    /// quedan guardadas en `split_ratio`/`v_ratio`.
    pub fn drag_divider(&mut self, divider: Divider, delta: f32, extent: f32) {
        let min = (MIN_PANEL_W / extent).clamp(0.05, 0.45);
        match divider {
            Divider::Vertical => {
                self.split_ratio = (self.split_ratio + delta / extent).clamp(min, 1.0 - min)
            }
            Divider::Horizontal => {
                // La fila necesita sitio para su barra de pestañas además del editor.
                let min_row = ((MIN_ROW_H / extent).clamp(0.05, 0.45)).max(min);
                self.v_ratio = (self.v_ratio + delta / extent).clamp(min_row, 1.0 - min_row);
            }
        }
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
                editor.set_show_whitespace(prefs.show_whitespace);
                editor.set_trim_on_save(prefs.trim_on_save);
                editor.set_font_size(prefs.font_size);
                editor.set_translate_tabs_to_spaces(prefs.translate_tabs_to_spaces);
                editor.set_auto_match(prefs.auto_match);
                editor.set_word_wrap(prefs.word_wrap);
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
        let was_focused = self.groups[gi].list[index].focused();
        let closed_path = self.groups[gi].list[index].path().map(|p| p.to_path_buf());
        if self.groups[gi].list.len() == 1 {
            if self.groups.len() == 1 {
                return false;
            }
            // En rejilla la celda no puede desaparecer (rompería el reparto):
            // se queda con un editor en blanco. En un split de 2 paneles sí
            // se quita el grupo y se vuelve a una celda única.
            if self.panes == Layout::Grid {
                let mut blank = Editor::default();
                blank.set_focused(true);
                self.groups[gi] = Group {
                    list: vec![blank],
                    active: 0,
                };
                self.remember_closed(closed_path);
                return true;
            }
            self.groups.remove(gi);
            if self.active_group >= self.groups.len() {
                self.active_group = self.groups.len() - 1;
            }
            self.panes = Layout::Single;
            self.active_mut().set_focused(true);
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
        // Si se cerró el editor que tenía el foco, la nueva activa lo
        // hereda: la ventana no debe quedarse sin caret (M26).
        if was_focused {
            group.list[group.active].set_focused(true);
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

    /// Barras de pestañas (una por celda) en la capa superior, más los
    /// divisores entre celdas; guarda el layout para [`Tabs::hit`]. Las que
    /// no caben no se dibujan ni se clican. Solo la pestaña activa del grupo
    /// CON FOCO se pinta como activa (M26); la × se muestra en ella y en la
    /// que está bajo el ratón, y la etiqueta de cada pestaña se omite si la
    /// tapa el menú de la barra de título (`set_occluded`).
    pub fn render(
        &mut self,
        scene: &mut Scene,
        viewport: (f32, f32),
        advance: f32,
        content_top: f32,
        content_h: f32,
    ) {
        let cells = self.group_geometry(viewport.0, content_top, content_h);
        // El hover se resuelve con el layout del render ANTERIOR: este
        // frame lo rellena más abajo (limpiarlo antes lo dejaría en None).
        let hovered = self.tab_at_pointer();
        self.layout.clear();

        for (gi, group) in self.groups.iter().enumerate() {
            let (gx, gy, gw, _gh) = cells[gi];
            let bar_right = gx + gw;
            let top = gy;
            // El grupo pierde el "aspecto enfocado" si su editor activo no
            // tiene foco de ventana (pestaña activa → como una inactiva).
            let group_focused = group.list[group.active].focused();
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

            let mut gl = GroupLayout {
                x: gx,
                y: gy,
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
                // La × de cierre se resta SIEMPRE del hueco de la etiqueta
                // (nada se recoloca al hacer hover).
                let w = (label.chars().count() as f32 * advance + 2.0 * TAB_PAD + TAB_CLOSE_W)
                    .clamp(TAB_MIN_W, TAB_MAX_W)
                    .min(bar_right - x);
                let is_active = index == group.active;
                let is_focused = is_active && group_focused;
                let is_hovered = hovered == Some((gi, index));
                let bg = if is_focused {
                    self.theme.tab_active_bg
                } else if is_hovered {
                    self.theme.tab_hover_bg
                } else {
                    self.theme.tab_bg
                };
                scene.top_quads.push(Quad::new(x, top, w, TAB_BAR_H, bg));
                // Sin borde inferior en la activa enfocada: se funde con su
                // editor; las demás (incluida la activa sin foco) lo llevan.
                if !is_focused {
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
                let capacity = ((w - 2.0 * TAB_PAD - TAB_CLOSE_W) / label_advance)
                    .floor()
                    .max(1.0) as usize;
                if label.chars().count() > capacity {
                    let head: String = label.chars().take(capacity.saturating_sub(1)).collect();
                    label = format!("{head}\u{2026}");
                }
                // Etiqueta y × NO se dibujan si las tapa el menú desplegado
                // de la barra de título: los textos van por encima de TODOS
                // los quads, así que la única solución es omitirlas (M26).
                let label_x = x + TAB_PAD;
                let label_y = top + (TAB_BAR_H - font) / 2.0;
                let label_w = label.chars().count() as f32 * label_advance;
                if !covered(self.occluded, label_x, label_y, label_w, font) {
                    scene.top_texts.push(TextSpan::new(
                        label,
                        label_x,
                        label_y,
                        font,
                        if is_focused {
                            self.theme.accent
                        } else {
                            self.theme.dim
                        },
                    ));
                }
                // × de cierre: en la activa enfocada y en la bajo el ratón
                // (la ZONA de click existe aunque no se dibuje).
                if (is_focused || is_hovered) && w >= TAB_CLOSE_W {
                    let close_x = x + w - TAB_CLOSE_W;
                    if !covered(self.occluded, close_x, label_y, TAB_CLOSE_W, font) {
                        scene.top_texts.push(TextSpan::new(
                            "\u{d7}".to_string(),
                            close_x + (TAB_CLOSE_W - label_advance) / 2.0,
                            label_y,
                            font,
                            if is_focused {
                                self.theme.accent
                            } else {
                                self.theme.fg
                            },
                        ));
                    }
                }

                gl.tabs.push((x, w));
                x += w;
            }
            self.layout.push(gl);
        }

        // Divisores entre celdas, por ENCIMA de las barras: verticales entre
        // columnas (a todo lo alto) y horizontales entre filas (a todo el
        // ancho). Solo hay una frontera por eje: las columnas comparten
        // `split_ratio` y las filas `v_ratio`.
        let (cols, rows) = self.panes.dims();
        if cols > 1 {
            let bx = viewport.0 * self.split_ratio;
            scene.top_quads.push(Quad::new(
                bx - 1.0,
                0.0,
                1.0,
                viewport.1,
                self.theme.tab_edge,
            ));
        }
        if rows > 1 {
            let by = content_top + content_h * self.v_ratio;
            scene.top_quads.push(Quad::new(
                0.0,
                by - 1.0,
                viewport.0,
                1.0,
                self.theme.tab_edge,
            ));
        }
    }

    /// Zona de la barra bajo `(x, y)`: la × de cierre si cae en su franja
    /// derecha (`TAB_CLOSE_W`), si no la propia pestaña. `None` si no fue
    /// dentro de la barra de SU celda o de una pestaña dibujada.
    pub fn hit(&self, x: f32, y: f32) -> Option<TabHit> {
        let group = self
            .layout
            .iter()
            .position(|gl| y >= gl.y && y < gl.y + TAB_BAR_H && x >= gl.x && x < gl.x + gl.w)?;
        let tab = self.layout[group]
            .tabs
            .iter()
            .position(|&(tx, w)| x >= tx && x < tx + w)?;
        let (tx, w) = self.layout[group].tabs[tab];
        if w >= TAB_CLOSE_W && x >= tx + w - TAB_CLOSE_W {
            Some(TabHit::Close(group, tab))
        } else {
            Some(TabHit::Select(group, tab))
        }
    }

    /// (grupo, pestaña) bajo el puntero de [`Tabs::set_pointer`]: al
    /// variar, `main.rs` repinta para mostrar u ocultar la ×.
    pub fn tab_at_pointer(&self) -> Option<(usize, usize)> {
        self.hit(self.pointer.0, self.pointer.1)
            .map(|hit| match hit {
                TabHit::Select(group, tab) | TabHit::Close(group, tab) => (group, tab),
            })
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
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        // Fondo + borde inferior + (bg + borde + separador + bg + separador).
        assert!(
            scene.top_quads.len() >= 6,
            "fondo + tabs: {}",
            scene.top_quads.len()
        );
        // Dos etiquetas + la × de la pestaña activa (M26).
        assert_eq!(scene.top_texts.len(), 3, "etiquetas + × de cierre");
        assert_eq!(
            scene
                .top_texts
                .iter()
                .filter(|t| t.text == "\u{d7}")
                .count(),
            1,
            "solo la activa lleva ×"
        );

        let tab0 = tabs.layout[0].tabs[0];
        let tab1 = tabs.layout[0].tabs[1];
        assert_eq!(
            tabs.hit(tab0.0 + tab0.1 / 2.0, TAB_BAR_H / 2.0),
            Some(TabHit::Select(0, 0))
        );
        assert_eq!(
            tabs.hit(tab1.0 + tab1.1 / 2.0, TAB_BAR_H / 2.0),
            Some(TabHit::Select(0, 1))
        );
        assert_eq!(tabs.hit(0.0, TAB_BAR_H + 1.0), None, "bajo la barra");
        assert_eq!(tabs.hit(799.0, 5.0), None, "pestaña que no cabe dibujada");

        // Muchas pestañas: no se dibuja ni se clica lo que no cabe.
        for i in 0..40 {
            tabs.add(tab_with(&format!("archivo_{i}")));
        }
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (300.0, 600.0), 8.0, 0.0, 600.0);
        assert!(tabs.layout[0].tabs.len() < 40, "solo caben unas pocas");
        assert!(tabs.layout[0].tabs.iter().all(|&(x, w)| x + w <= 300.0));
    }

    #[test]
    fn close_button_zone_is_always_reserved_and_reports_close() {
        let mut tabs = Tabs::new(tab_with("archivo_muy_largo"));
        tabs.add(tab_with("otro.rs"));

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        let (tx, w) = tabs.layout[0].tabs[0];
        // La franja derecha (16 px) es la × aunque NO se dibuje en inactivas.
        assert_eq!(
            tabs.hit(tx + w - 4.0, TAB_BAR_H / 2.0),
            Some(TabHit::Close(0, 0)),
            "últimos 16 px = cerrar"
        );
        assert_eq!(
            tabs.hit(tx + 4.0, TAB_BAR_H / 2.0),
            Some(TabHit::Select(0, 0)),
            "el resto = seleccionar"
        );
        // La × de la activa se dibuja siempre; la de la otra, con hover.
        let x_marks = |scene: &Scene| {
            scene
                .top_texts
                .iter()
                .filter(|t| t.text == "\u{d7}")
                .count()
        };
        assert_eq!(x_marks(&scene), 1, "solo la activa sin hover");

        let (hx, hw) = tabs.layout[0].tabs[0];
        tabs.set_pointer(hx + hw / 2.0, TAB_BAR_H / 2.0);
        assert_eq!(tabs.tab_at_pointer(), Some((0, 0)), "hover de la inactiva");
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        assert_eq!(x_marks(&scene), 2, "hover muestra su ×");

        // Fuera de la barra no hay hover (y la × de la otra desaparece).
        tabs.set_pointer(-1.0, -1.0);
        assert_eq!(tabs.tab_at_pointer(), None);
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        assert_eq!(x_marks(&scene), 1, "sin puntero solo la activa");
    }

    #[test]
    fn active_tab_of_an_unfocused_group_looks_inactive() {
        let mut tabs = Tabs::new(tab_with("uno"));
        tabs.split(); // grupo 1 activo y con foco; el 0 queda apagado
        assert!(!tabs.group_active_mut(0).focused(), "grupo 0 sin foco");

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        // Grupo 0: barra, filete, pestaña activa (bg inactivo + su filete).
        assert_eq!(scene.top_quads[2].color, Theme::dark().tab_bg);
        assert_eq!(scene.top_quads[3].color, Theme::dark().tab_edge);
        // El grupo 1 (con foco) sí funde su pestaña activa con el editor.
        assert!(
            scene
                .top_quads
                .iter()
                .any(|q| q.color == Theme::dark().tab_active_bg),
            "la activa enfocada se funde"
        );
        // Etiquetas: la del grupo apagado va en `dim`, la enfocada en acento.
        assert_eq!(scene.top_texts[0].color, Theme::dark().dim);
        assert_eq!(scene.top_texts[1].color, Theme::dark().accent);
        // Y solo la enfocada lleva ×.
        assert_eq!(
            scene
                .top_texts
                .iter()
                .filter(|t| t.text == "\u{d7}")
                .count(),
            1
        );

        // Devolverle el foco al grupo 0 reactiva su pestaña activa.
        tabs.set_active_group(0);
        tabs.active_mut().set_focused(true);
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
        assert_eq!(scene.top_quads[2].color, Theme::dark().tab_active_bg);
    }

    #[test]
    fn labels_under_the_open_menu_rect_are_not_drawn() {
        let mut tabs = Tabs::new(tab_named("uno.rs"));
        tabs.add(tab_named("dos.rs"));
        // Como en la app: la barra empieza bajo la barra de título (y=30).
        let (ct, ch) = (30.0, 570.0);

        // Menú que tapa la barra entera: no se dibuja NINGUNA etiqueta.
        tabs.set_occluded(Some((0.0, 30.0, 800.0, 242.0)));
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, ct, ch);
        assert!(
            scene.top_texts.is_empty(),
            "las etiquetas no asoman sobre el menú: {:?}",
            scene
                .top_texts
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
        );
        // Los quads sí se dibujan (el fondo del menú los tapa después).
        assert!(scene.top_quads.len() >= 6, "los quads siguen ahí");

        // Un menú estrecho a la izquierda tapa solo la primera etiqueta.
        tabs.set_occluded(Some((0.0, 30.0, 60.0, 190.0)));
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, ct, ch);
        let labels: Vec<&str> = scene
            .top_texts
            .iter()
            .filter(|t| t.text != "\u{d7}")
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(labels, vec!["dos.rs"], "la primera está tapada");

        // Sin menú se dibuja todo (regresión del default).
        tabs.set_occluded(None);
        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (800.0, 600.0), 8.0, ct, ch);
        assert_eq!(
            scene
                .top_texts
                .iter()
                .filter(|t| t.text != "\u{d7}")
                .count(),
            2
        );
    }

    #[test]
    fn closing_the_focused_tab_hands_focus_to_the_new_active() {
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.add(tab_named("b.rs"));
        // Solo la activa (b) tiene foco, como tras perder/recuperar la
        // ventana con dos pestañas abiertas.
        tabs.set_active(0);
        tabs.active_mut().set_focused(false);
        tabs.set_active(1);
        tabs.active_mut().set_focused(true);
        assert!(tabs.iter().nth(1).is_some_and(|e| e.focused()));

        assert!(tabs.close(None), "cierra b, la enfocada");
        assert_eq!(tabs.len(), 1);
        assert!(
            tabs.active().focused(),
            "la nueva activa hereda el foco (sin caret no hay editor)"
        );
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

        tabs.set_group_geometry(1, 500.0, 0.0, 500.0, 600.0);
        assert_eq!(tabs.group_active_mut(1).left_inset(), 500.0);

        let mut scene = Scene::new(wgpu::Color::BLACK);
        tabs.render(&mut scene, (1000.0, 600.0), 8.0, 0.0, 600.0);
        assert_eq!(tabs.layout.len(), 2, "una franja por grupo");
        assert_eq!(tabs.layout[0].x, 0.0);
        assert_eq!(tabs.layout[1].x, 500.0, "la segunda columna empieza a 500");
        // Etiqueta de cada grupo + × de la activa del grupo con foco (M26).
        assert_eq!(scene.top_texts.len(), 3, "una etiqueta por grupo + ×");
        assert!(
            scene.top_quads.iter().any(|q| q.h == 600.0 && q.w == 1.0),
            "separador vertical a altura completa"
        );

        // El click resuelve (grupo, pestaña) por la x dentro de cada barra;
        // el hueco vacío de la barra no es clicable.
        assert_eq!(tabs.hit(50.0, TAB_BAR_H / 2.0), Some(TabHit::Select(0, 0)));
        assert_eq!(tabs.hit(550.0, TAB_BAR_H / 2.0), Some(TabHit::Select(1, 0)));
        assert_eq!(tabs.hit(700.0, TAB_BAR_H / 2.0), None, "hueco vacío");
        assert_eq!(tabs.hit(700.0, TAB_BAR_H + 1.0), None, "bajo la barra");

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
            word_wrap: true,
            ..crate::settings::Settings::default()
        };
        tabs.apply_prefs(&prefs);

        for gi in 0..tabs.group_count() {
            let editor = tabs.group_active_mut(gi);
            assert_eq!(editor.tab_width(), 8, "grupo {gi} actualizado");
            assert!(!editor.show_line_numbers(), "grupo {gi} sin gutter");
            assert!(editor.translate_tabs_to_spaces(), "grupo {gi} con espacios");
            assert!(!editor.auto_match(), "grupo {gi} sin auto_match");
            assert!(editor.word_wrap(), "grupo {gi} con ajuste de línea");
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
        tabs.render(&mut scene, (800.0, 600.0), 8.0, 0.0, 600.0);
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
        tabs.render(&mut scene, (800.0, 600.0), 12.0, 0.0, 600.0);
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
        tabs.render(&mut scene, (800.0, 600.0), 16.0, 0.0, 600.0);
        let label = scene.top_texts.first().expect("etiqueta dibujada");
        assert_eq!(label.size, 24.0, "tope por la altura de la barra");
    }

    #[test]
    fn group_geometry_splits_by_ratio_and_divider_follows_it() {
        let mut tabs = Tabs::new(Editor::default());
        let (ct, ch) = (0.0, 600.0);
        assert_eq!(
            tabs.group_geometry(1000.0, ct, ch),
            vec![(0.0, 0.0, 1000.0, 600.0)],
            "un solo grupo ocupa todo el ancho"
        );
        assert_eq!(tabs.group_at(500.0, 300.0, 1000.0, ct, ch), 0);
        assert!(
            tabs.over_divider(500.0, 300.0, 1000.0, ct, ch).is_none(),
            "sin split no hay divisor"
        );

        tabs.split();
        assert_eq!(
            tabs.group_geometry(1000.0, ct, ch),
            vec![(0.0, 0.0, 500.0, 600.0), (500.0, 0.0, 500.0, 600.0)],
            "reparto por defecto 50/50"
        );
        assert_eq!(
            tabs.over_divider(500.0, 300.0, 1000.0, ct, ch),
            Some(Divider::Vertical),
            "sobre el eje"
        );
        assert_eq!(
            tabs.over_divider(503.0, 300.0, 1000.0, ct, ch),
            Some(Divider::Vertical),
            "el agarre es de pocos px"
        );
        assert_eq!(
            tabs.over_divider(510.0, 300.0, 1000.0, ct, ch),
            None,
            "fuera del agarre"
        );
        assert_eq!(
            tabs.group_at(501.0, 300.0, 1000.0, ct, ch),
            1,
            "click a la derecha"
        );

        // 0.5 + 125/1000 = 0.625 (exacto en binario): 625/375.
        tabs.drag_divider(Divider::Vertical, 125.0, 1000.0);
        assert_eq!(
            tabs.group_geometry(1000.0, ct, ch),
            vec![(0.0, 0.0, 625.0, 600.0), (625.0, 0.0, 375.0, 600.0)],
            "el divisor sigue al arrastre"
        );
        assert_eq!(
            tabs.over_divider(625.0, 300.0, 1000.0, ct, ch),
            Some(Divider::Vertical),
            "el divisor se mueve"
        );

        tabs.drag_divider(Divider::Vertical, -10_000.0, 1000.0);
        let left = tabs.group_geometry(1000.0, ct, ch)[0].2;
        assert!(
            (left - 80.0).abs() < 0.01,
            "ancho mínimo izquierdo, got {left}"
        );
        tabs.drag_divider(Divider::Vertical, 10_000.0, 1000.0);
        let right = tabs.group_geometry(1000.0, ct, ch)[1].2;
        assert!(
            (right - 80.0).abs() < 0.01,
            "ancho mínimo derecho, got {right}"
        );
    }

    #[test]
    fn unsplit_remembers_the_ratio_for_the_next_split() {
        let mut tabs = Tabs::new(Editor::default());
        let (ct, ch) = (0.0, 600.0);
        tabs.split();
        tabs.drag_divider(Divider::Vertical, 100.0, 1000.0); // 0.5 + 0.1 = 0.6

        tabs.unsplit();
        assert_eq!(
            tabs.group_geometry(1200.0, ct, ch),
            vec![(0.0, 0.0, 1200.0, 600.0)],
            "sin split vuelve a un panel"
        );
        assert!(tabs.over_divider(600.0, 300.0, 1200.0, ct, ch).is_none());

        tabs.split();
        let left = tabs.group_geometry(1000.0, ct, ch)[0].2;
        assert!(
            (left - 600.0).abs() < 0.01,
            "recuerda el reparto arrastrado"
        );
    }

    #[test]
    fn rows_and_grid_layouts_split_on_both_axes() {
        let mut tabs = Tabs::new(tab_with("uno"));
        let (ct, ch) = (0.0, 600.0);

        // Dos filas: división vertical (eje Y) por `v_ratio`.
        tabs.set_layout(1, 2);
        assert_eq!(tabs.layout(), (1, 2));
        assert_eq!(tabs.group_count(), 2);
        let cells = tabs.group_geometry(1000.0, ct, ch);
        assert_eq!(cells[0], (0.0, 0.0, 1000.0, 300.0), "fila superior 50%");
        assert_eq!(cells[1], (0.0, 300.0, 1000.0, 300.0), "fila inferior");
        assert_eq!(
            tabs.over_divider(500.0, 300.0, 1000.0, ct, ch),
            Some(Divider::Horizontal),
            "divisor horizontal sobre el eje Y"
        );
        assert_eq!(
            tabs.group_at(500.0, 350.0, 1000.0, ct, ch),
            1,
            "click en la fila inferior"
        );

        // Rejilla 2×2: cuatro celdas, ambos ejos.
        tabs.set_layout(2, 2);
        assert_eq!(tabs.layout(), (2, 2));
        assert_eq!(
            tabs.group_count(),
            4,
            "las celdas nuevas empiezan en blanco"
        );
        let cells = tabs.group_geometry(1000.0, ct, ch);
        assert_eq!(cells[0], (0.0, 0.0, 500.0, 300.0));
        assert_eq!(cells[3], (500.0, 300.0, 500.0, 300.0));
        assert_eq!(
            tabs.over_divider(500.0, 100.0, 1000.0, ct, ch),
            Some(Divider::Vertical),
            "frontera de columnas"
        );
        assert_eq!(
            tabs.over_divider(200.0, 300.0, 1000.0, ct, ch),
            Some(Divider::Horizontal),
            "frontera de filas"
        );

        // Volver a una celda fusiona sin perder pestañas.
        tabs.set_layout(1, 1);
        assert_eq!(tabs.layout(), (1, 1));
        assert_eq!(tabs.group_count(), 1);
        assert!(tabs.len() >= 1, "las pestañas se conservan");
    }

    #[test]
    fn grid_close_keeps_the_cell_but_column_close_drops_it() {
        let mut tabs = Tabs::new(tab_named("uno.rs"));
        tabs.split();
        assert_eq!(tabs.group_count(), 2);
        assert!(tabs.close(None), "cierra la única pestaña del grupo");
        assert_eq!(tabs.group_count(), 1, "el grupo derecho desaparece");
        assert_eq!(tabs.layout(), (1, 1), "volvemos a una celda única");

        // En rejilla la celda no puede desaparecer: queda en blanco.
        let mut tabs = Tabs::new(tab_named("a.rs"));
        tabs.set_layout(2, 2);
        tabs.set_active_group(3);
        assert!(tabs.close(None), "cierra la pestaña de la celda");
        assert_eq!(tabs.group_count(), 4, "la celda sigue ahí");
        assert_eq!(tabs.layout(), (2, 2));
        assert!(!tabs.active().has_path(), "la celda queda en blanco");
    }
}
