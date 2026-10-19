//! Minimap: vista previa lateral del buffer (M27).
//!
//! Reserva la franja derecha de cada celda con una versión en miniatura del
//! código (2 px por línea, con los colores de sintaxis) y un rectángulo con
//! la ventana visible, arrastrable para hacer scroll. La geometría vive aquí
//! para que render, ratón y tests compartan la misma fuente (igual que
//! [`crate::title::TitleBar::drop_geometry`]).

use crate::tabs::TAB_BAR_H;

/// Ancho de la franja del minimap (px lógicos).
pub const MINIMAP_W: f32 = 100.0;
/// Altura de cada línea en el minimap (px lógicos).
pub const LINE_PITCH: f32 = 2.0;
/// Tamaño de la fuente en el minimap (px lógicos): las líneas de 2 px
/// caben texto de ~80 columnas en la franja.
pub const FONT: f32 = 2.0;
/// Margen horizontal dentro de la franja (px lógicos).
pub const PAD_X: f32 = 3.0;
/// Ancho mínimo de la celda para reservar el minimap: por debajo taparía
/// más código del que aporta (paneles estrechos se quedan sin franja).
const MIN_CELL_W: f32 = MINIMAP_W + 180.0;

/// Líneas del buffer que caben en una franja de alto `h`.
pub fn lines_fit(h: f32) -> usize {
    ((h / LINE_PITCH).floor() as usize).max(1)
}

/// Primera línea del buffer que muestra el minimap: si el archivo cabe
/// entero (`total <= fit`) empieza en 0; si no, la ventana se desplaza de
/// forma **proporcional** al scroll del editor (un arrastre reparte todas
/// las líneas del archivo sobre la franja).
pub fn map_first(total: usize, fit: usize, scroll_line: usize, view_lines: usize) -> usize {
    if total <= fit {
        return 0;
    }
    let max_first = total - fit;
    // Posición del scroll dentro del rango recorrible (sin pasar del fin).
    let denom = total.saturating_sub(view_lines).max(1);
    let pos = scroll_line.min(denom);
    // Redondeo al entero más cercano para que los extremos casen exactos.
    (pos * max_first + denom / 2) / denom
}

/// Línea del buffer bajo `y_rel` píxeles dentro de la franja (el `first`
/// congelado evita que el mapa se deslice bajo el ratón mientras se arrastra).
pub fn line_at(y_rel: f32, first: usize, total: usize, fit: usize) -> usize {
    let rel = (y_rel / LINE_PITCH).floor().max(0.0) as usize;
    (first + rel).min(total.saturating_sub(1)).min(first + fit)
}

/// Estado del mapeo buffer ↔ minimap en un instante (lo calcula
/// [`crate::editor::Editor::minimap_view`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Franja `(x, y, w, h)` en coordenadas de ventana.
    pub rect: (f32, f32, f32, f32),
    /// Primera línea del buffer visible en la franja.
    pub first: usize,
    /// Total de líneas del buffer.
    pub total: usize,
    /// Líneas que caben en la franja.
    pub fit: usize,
    /// Primera línea lógica visible en el EDITOR (scroll actual).
    pub scroll_line: usize,
    /// Líneas visibles en el viewport del editor.
    pub view_lines: usize,
}

impl View {
    /// Coordenada `y` de ventana → píxeles dentro de la franja.
    pub fn y_rel(&self, py: f32) -> f32 {
        py - self.rect.1
    }
}

/// Rectángulo del viewport dentro de la franja `(x, y, w, h)` absoluto:
/// sigue a `scroll_line` y se mantiene dentro de la franja (un archivo
/// corto muestra el rect a tamaño completo).
pub fn viewport_rect(view: &View) -> (f32, f32, f32, f32) {
    let (x, y, _w, h) = view.rect;
    let rel = view.scroll_line.saturating_sub(view.first) as f32 * LINE_PITCH;
    let full = view.view_lines as f32 * LINE_PITCH;
    let rect_h = full.clamp(4.0, h).min(h);
    let rect_y = rel.clamp(0.0, (h - rect_h).max(0.0));
    (x, y + rect_y, MINIMAP_W, rect_h)
}

/// Franja del minimap dentro de una celda del split, o `None` si la celda
/// es demasiado estrecha (o alta: la barra de pestañas se resta).
pub fn rect(cell: (f32, f32, f32, f32)) -> Option<(f32, f32, f32, f32)> {
    let (x, y, w, h) = cell;
    if w < MIN_CELL_W {
        return None;
    }
    let editor_h = (h - TAB_BAR_H).max(50.0);
    Some((x + w - MINIMAP_W, y + TAB_BAR_H, MINIMAP_W, editor_h))
}

/// Índice de la celda cuya franja contiene `(px, py)`.
pub fn group_at(cells: &[(f32, f32, f32, f32)], px: f32, py: f32) -> Option<usize> {
    cells.iter().position(|&cell| {
        rect(cell).is_some_and(|(x, y, w, h)| px >= x && px < x + w && py >= y && py < y + h)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fit_scales_with_height_and_never_zeroes() {
        assert_eq!(lines_fit(600.0), 300);
        assert_eq!(lines_fit(1.5), 1, "suelo de una línea");
    }

    #[test]
    fn small_files_pin_the_window_at_zero() {
        assert_eq!(map_first(40, 300, 0, 30), 0);
        assert_eq!(map_first(40, 300, 39, 30), 0, "cabe entero");
    }

    #[test]
    fn big_files_map_the_scroll_proportionally() {
        let (total, fit, view) = (10_000, 300, 30);
        assert_eq!(map_first(total, fit, 0, view), 0, "arriba → inicio");
        assert_eq!(
            map_first(total, fit, total - view, view),
            total - fit,
            "abajo → final"
        );
        // Monótono: más scroll no puede devolver una ventana anterior.
        let mut prev = 0;
        for scroll in (0..total).step_by(137) {
            let first = map_first(total, fit, scroll, view);
            assert!(first >= prev, "monótono en {scroll}");
            assert!(first + fit <= total, "ventana dentro del archivo");
            prev = first;
        }
    }

    #[test]
    fn line_at_reads_lines_and_clamps_to_the_end() {
        assert_eq!(line_at(0.0, 10, 1_000, 300), 10);
        assert_eq!(line_at(19.9, 10, 1_000, 300), 19, "2 px por línea");
        assert_eq!(line_at(-50.0, 10, 1_000, 300), 10, "suelo en first");
        assert_eq!(line_at(9_999.0, 10, 1_000, 300), 310, "techo first+fit");
        assert_eq!(line_at(9_999.0, 990, 1_000, 300), 999, "techo total-1");
    }

    #[test]
    fn viewport_rect_follows_the_scroll_and_stays_inside() {
        let view = View {
            rect: (500.0, 30.0, MINIMAP_W, 570.0),
            first: 100,
            total: 1_000,
            fit: 285,
            scroll_line: 100,
            view_lines: 30,
        };
        let (x, y, w, h) = viewport_rect(&view);
        assert_eq!((x, w), (500.0, MINIMAP_W), "pegada a la derecha");
        assert_eq!(y, 30.0, "arriba cuando el scroll coincide");
        assert_eq!(h, 60.0, "30 líneas × 2 px");

        // Con scroll lejano el rect no se sale de la franja por abajo.
        let far = View {
            scroll_line: 999,
            ..view
        };
        let (_, y2, _, h2) = viewport_rect(&far);
        assert!(
            y2 + h2 <= 30.0 + 570.0 + 0.001,
            "clamp al fondo de la franja"
        );
    }

    #[test]
    fn rect_reserves_the_right_edge_below_the_tab_bar() {
        // Celda ancha: franja pegada a la derecha, bajo la barra.
        let (x, y, w, h) = rect((100.0, 62.0, 600.0, 500.0)).expect("celda ancha");
        assert_eq!(x, 100.0 + 600.0 - MINIMAP_W);
        assert_eq!(y, 62.0 + TAB_BAR_H, "bajo la barra de pestañas");
        assert_eq!(w, MINIMAP_W);
        assert_eq!(h, 500.0 - TAB_BAR_H);

        // Celda estrecha: sin minimap (no tapa el código).
        assert!(rect((0.0, 62.0, 279.9, 500.0)).is_none());
    }

    #[test]
    fn group_at_hits_only_the_fringe_of_its_own_cell() {
        let cells = [
            (0.0, 62.0, 400.0, 300.0),   // grupo 0 (con minimap)
            (401.0, 62.0, 200.0, 300.0), // grupo 1 (demasiado estrecho)
        ];
        assert_eq!(group_at(&cells, 350.0, 100.0), Some(0), "en la franja");
        assert_eq!(group_at(&cells, 250.0, 100.0), None, "sobre el código");
        assert_eq!(group_at(&cells, 550.0, 100.0), None, "celda estrecha");
        assert_eq!(group_at(&cells, 350.0, 400.0), None, "fuera en y");
    }
}
