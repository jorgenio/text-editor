//! Temas de color de la interfaz: OSCURO (Monokai, el original y por
//! defecto) y CLARO (fondo blanco con texto oscuro).
//!
//! Un tema cubre el editor, la barra de pestañas y la barra de buscar. La
//! paleta de comandos y los avisos efímeros son overlays y se quedan
//! oscuros en ambos temas. `settings.json` guarda `"theme": "dark" | "light"`
//! y Ctrl+T (o "Cambiar tema claro/oscuro" en la paleta) alterna entre ellos.
//!
//! Los valores del tema oscuro son EXACTAMENTE los colores históricos de
//! editor/tabs/search: cambiar de tema no altera nada por defecto.

use jorgenio_syntax::SyntaxThemeKind;

/// Colores completos de la interfaz para un frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Paleta de syntect (Monokai / InspiredGitHub).
    pub syntax: SyntaxThemeKind,
    // — Editor —
    pub editor_bg: (u8, u8, u8),
    /// Texto sin resaltar (y etiquetas de pestaña activa y de la barra).
    pub fg: (u8, u8, u8),
    /// Texto apagado (contadores, pestañas inactivas).
    pub dim: (u8, u8, u8),
    pub gutter_fg: (u8, u8, u8),
    pub gutter_current: (u8, u8, u8),
    pub caret: (u8, u8, u8, u8),
    pub selection: (u8, u8, u8, u8),
    pub match_bg: (u8, u8, u8, u8),
    pub current_match: (u8, u8, u8, u8),
    // — Barra de pestañas —
    pub bar_bg: (u8, u8, u8, u8),
    pub tab_bg: (u8, u8, u8, u8),
    /// Pestaña activa: mismo tono que `editor_bg` (se funde con él).
    pub tab_active_bg: (u8, u8, u8, u8),
    pub tab_edge: (u8, u8, u8, u8),
    // — Barra de buscar/reemplazar —
    pub panel_bg: (u8, u8, u8, u8),
    pub panel_edge: (u8, u8, u8, u8),
    pub field_bg: (u8, u8, u8, u8),
    pub button_bg: (u8, u8, u8, u8),
}

impl Theme {
    /// Monokai oscuro: los colores originales del editor, la barra de
    /// pestañas y la barra de buscar, tal cual.
    pub fn dark() -> Self {
        Self {
            syntax: SyntaxThemeKind::Dark,
            editor_bg: (0x27, 0x28, 0x22),
            fg: (0xF8, 0xF8, 0xF2),
            dim: (0x75, 0x71, 0x5E),
            gutter_fg: (0x75, 0x71, 0x5E),
            gutter_current: (0xFD, 0x97, 0x1F),
            caret: (0xF8, 0xF8, 0xF2, 0xFF),
            selection: (0x49, 0x48, 0x3E, 0xFF),
            match_bg: (0x5A, 0x57, 0x44, 0xB0),
            current_match: (0x9A, 0x91, 0x58, 0xFF),
            bar_bg: (0x1A, 0x1B, 0x18, 0xFF),
            tab_bg: (0x22, 0x23, 0x1E, 0xFF),
            tab_active_bg: (0x27, 0x28, 0x22, 0xFF),
            tab_edge: (0x3E, 0x3D, 0x32, 0xFF),
            panel_bg: (0x1E, 0x1F, 0x1C, 0xFF),
            panel_edge: (0x3E, 0x3D, 0x32, 0xFF),
            field_bg: (0x14, 0x15, 0x12, 0xFF),
            button_bg: (0x49, 0x48, 0x3E, 0xFF),
        }
    }

    /// Fondo blanco con texto oscuro y resaltado InspiredGitHub.
    pub fn light() -> Self {
        Self {
            syntax: SyntaxThemeKind::Light,
            editor_bg: (0xFF, 0xFF, 0xFF),
            fg: (0x33, 0x33, 0x33),
            dim: (0x8A, 0x90, 0x99),
            gutter_fg: (0xA5, 0xAA, 0xB2),
            gutter_current: (0x1F, 0x23, 0x28),
            caret: (0x00, 0x00, 0x00, 0xFF),
            selection: (0xCC, 0xE2, 0xFF, 0xFF),
            match_bg: (0xF6, 0xE0, 0x9B, 0xB0),
            current_match: (0xF6, 0xC3, 0x45, 0xFF),
            bar_bg: (0xE9, 0xEA, 0xEC, 0xFF),
            tab_bg: (0xD5, 0xD8, 0xDC, 0xFF),
            tab_active_bg: (0xFF, 0xFF, 0xFF, 0xFF),
            tab_edge: (0xC5, 0xC9, 0xCE, 0xFF),
            panel_bg: (0xF3, 0xF4, 0xF6, 0xFF),
            panel_edge: (0xC5, 0xC9, 0xCE, 0xFF),
            field_bg: (0xFF, 0xFF, 0xFF, 0xFF),
            button_bg: (0xDF, 0xE3, 0xE7, 0xFF),
        }
    }

    /// Tema por nombre de `settings.json` ("light"); todo lo demás → oscuro
    /// (el default y el valor normalizado de [`crate::settings::parse`]).
    pub fn from_name(name: &str) -> Self {
        if name == "light" {
            Self::light()
        } else {
            Self::dark()
        }
    }

    /// Etiqueta en español para el aviso efímero.
    pub fn label(&self) -> &'static str {
        if self.syntax == SyntaxThemeKind::Light {
            "claro"
        } else {
            "oscuro"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_exactly_the_legacy_monokai_palette() {
        let dark = Theme::dark();
        assert_eq!(dark.syntax, SyntaxThemeKind::Dark);
        assert_eq!(dark.editor_bg, (0x27, 0x28, 0x22));
        assert_eq!(dark.fg, (0xF8, 0xF8, 0xF2));
        assert_eq!(dark.dim, (0x75, 0x71, 0x5E));
        assert_eq!(dark.gutter_fg, (0x75, 0x71, 0x5E));
        assert_eq!(dark.gutter_current, (0xFD, 0x97, 0x1F));
        assert_eq!(dark.caret, (0xF8, 0xF8, 0xF2, 0xFF));
        assert_eq!(dark.selection, (0x49, 0x48, 0x3E, 0xFF));
        assert_eq!(dark.match_bg, (0x5A, 0x57, 0x44, 0xB0));
        assert_eq!(dark.current_match, (0x9A, 0x91, 0x58, 0xFF));
        assert_eq!(dark.bar_bg, (0x1A, 0x1B, 0x18, 0xFF));
        assert_eq!(dark.tab_bg, (0x22, 0x23, 0x1E, 0xFF));
        assert_eq!(dark.tab_active_bg, (0x27, 0x28, 0x22, 0xFF));
        assert_eq!(dark.tab_edge, (0x3E, 0x3D, 0x32, 0xFF));
        assert_eq!(dark.panel_bg, (0x1E, 0x1F, 0x1C, 0xFF));
        assert_eq!(dark.panel_edge, (0x3E, 0x3D, 0x32, 0xFF));
        assert_eq!(dark.field_bg, (0x14, 0x15, 0x12, 0xFF));
        assert_eq!(dark.button_bg, (0x49, 0x48, 0x3E, 0xFF));
    }

    #[test]
    fn light_is_bright_with_dark_text_and_other_syntax() {
        let light = Theme::light();
        let dark = Theme::dark();
        assert_eq!(light.syntax, SyntaxThemeKind::Light);
        let luma = |theme: Theme| {
            u32::from(theme.editor_bg.0)
                + u32::from(theme.editor_bg.1)
                + u32::from(theme.editor_bg.2)
        };
        assert!(
            luma(light) > luma(dark),
            "el claro debe ser más luminoso: {light:?}"
        );
        let fg_sum = u32::from(light.fg.0) + u32::from(light.fg.1) + u32::from(light.fg.2);
        assert!(fg_sum < 300, "texto oscuro sobre blanco: {light:?}");
        assert_ne!(light, dark);
    }

    #[test]
    fn active_tab_fuses_with_editor_bg_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let bg = (
                theme.editor_bg.0,
                theme.editor_bg.1,
                theme.editor_bg.2,
                0xFF,
            );
            assert_eq!(
                theme.tab_active_bg, bg,
                "la pestaña activa se funde con el editor"
            );
        }
    }

    #[test]
    fn from_name_defaults_to_dark() {
        assert_eq!(Theme::from_name("light"), Theme::light());
        assert_eq!(Theme::from_name("dark"), Theme::dark());
        assert_eq!(Theme::from_name(""), Theme::dark());
        assert_eq!(Theme::from_name("azul"), Theme::dark());
    }

    #[test]
    fn labels_and_syntax_kind_match() {
        assert_eq!(Theme::dark().label(), "oscuro");
        assert_eq!(Theme::light().label(), "claro");
        assert_eq!(Theme::from_name("light").syntax, SyntaxThemeKind::Light);
    }
}
