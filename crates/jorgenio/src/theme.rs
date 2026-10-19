//! Temas de color de la interfaz: OSCURO y CLARO, con la paleta inspirada
//! en Navicat (azul de acento sobre superficies frías grises y filetes azul
//! acero): el mismo esquema de `dbstudio-app/src/theme.rs` del proyecto
//! vecino, mapeado campo a campo.
//!
//! Un tema cubre TODO el chrome: editor, pestañas, barra de título y su
//! menú, barra de buscar, paleta de comandos, barra de estado y avisos.
//! `settings.json` guarda `"theme": "dark" | "light"` y Ctrl+T (o "Cambiar
//! tema claro/oscuro" en la paleta) alterna entre ellos.
//!
//! El resaltado de sintaxis no lo pinta este módulo: sigue siendo Monokai
//! (oscuro) e InspiredGitHub (claro), de syntect.

use jorgenio_syntax::SyntaxThemeKind;

/// Colores completos de la interfaz para un frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Paleta de syntect (Monokai / InspiredGitHub).
    pub syntax: SyntaxThemeKind,
    // — Editor —
    pub editor_bg: (u8, u8, u8),
    /// Texto sin resaltar (etiquetas de la barra de título, títulos).
    pub fg: (u8, u8, u8),
    /// Texto apagado (contadores, pestañas inactivas).
    pub dim: (u8, u8, u8),
    /// Texto de error (líneas de stderr en el panel de build, M28).
    pub error: (u8, u8, u8),
    /// Azul de acento (Navicat `primary`): etiqueta de la pestaña activa,
    /// número de línea actual y base de caret/coincidencias.
    pub accent: (u8, u8, u8),
    pub gutter_fg: (u8, u8, u8),
    pub gutter_current: (u8, u8, u8),
    pub caret: (u8, u8, u8, u8),
    pub selection: (u8, u8, u8, u8),
    pub match_bg: (u8, u8, u8, u8),
    pub current_match: (u8, u8, u8, u8),
    // — Barra de pestañas —
    pub bar_bg: (u8, u8, u8, u8),
    pub tab_bg: (u8, u8, u8, u8),
    /// Pestaña inactiva bajo el ratón.
    pub tab_hover_bg: (u8, u8, u8, u8),
    /// Pestaña activa: mismo tono que `editor_bg` (se funde con él).
    pub tab_active_bg: (u8, u8, u8, u8),
    pub tab_edge: (u8, u8, u8, u8),
    // — Paneles flotantes (buscar, paleta, menú) —
    pub panel_bg: (u8, u8, u8, u8),
    pub panel_edge: (u8, u8, u8, u8),
    pub field_bg: (u8, u8, u8, u8),
    pub button_bg: (u8, u8, u8, u8),
}

impl Theme {
    /// Oscuro: pane `1C2026`, chrome `14171C`/`171B21`, acento `5B8DEF`.
    pub fn dark() -> Self {
        Self {
            syntax: SyntaxThemeKind::Dark,
            editor_bg: (0x1C, 0x20, 0x26),
            fg: (0xD8, 0xDC, 0xE3),
            dim: (0x7D, 0x85, 0x92),
            error: (0xE5, 0x63, 0x63),
            accent: (0x5B, 0x8D, 0xEF),
            gutter_fg: (0x7D, 0x85, 0x92),
            gutter_current: (0x5B, 0x8D, 0xEF),
            caret: (0x5B, 0x8D, 0xEF, 0xFF),
            selection: (0x1F, 0x2D, 0x47, 0xFF),
            match_bg: (0x5B, 0x8D, 0xEF, 0x3A),
            current_match: (0x5B, 0x8D, 0xEF, 0x8C),
            bar_bg: (0x14, 0x17, 0x1C, 0xFF),
            tab_bg: (0x17, 0x1B, 0x21, 0xFF),
            tab_hover_bg: (0x1F, 0x24, 0x2C, 0xFF),
            tab_active_bg: (0x1C, 0x20, 0x26, 0xFF),
            tab_edge: (0x2C, 0x31, 0x3A, 0xFF),
            panel_bg: (0x19, 0x1D, 0x23, 0xFF),
            panel_edge: (0x2C, 0x31, 0x3A, 0xFF),
            field_bg: (0x14, 0x17, 0x1C, 0xFF),
            button_bg: (0x23, 0x29, 0x32, 0xFF),
        }
    }

    /// Claro: pane `FFFFFF`, chrome `F1F3F6`/`F5F6F9`, acento `2F6FED`.
    pub fn light() -> Self {
        Self {
            syntax: SyntaxThemeKind::Light,
            editor_bg: (0xFF, 0xFF, 0xFF),
            fg: (0x2E, 0x32, 0x38),
            dim: (0x8B, 0x91, 0x9C),
            error: (0xC0, 0x39, 0x2B),
            accent: (0x2F, 0x6F, 0xED),
            gutter_fg: (0x8B, 0x91, 0x9C),
            gutter_current: (0x2F, 0x6F, 0xED),
            caret: (0x2F, 0x6F, 0xED, 0xFF),
            selection: (0xE3, 0xEC, 0xFD, 0xFF),
            match_bg: (0x2F, 0x6F, 0xED, 0x3A),
            current_match: (0x2F, 0x6F, 0xED, 0x8C),
            bar_bg: (0xF1, 0xF3, 0xF6, 0xFF),
            tab_bg: (0xF5, 0xF6, 0xF9, 0xFF),
            tab_hover_bg: (0xEC, 0xEE, 0xF2, 0xFF),
            tab_active_bg: (0xFF, 0xFF, 0xFF, 0xFF),
            tab_edge: (0xDF, 0xE3, 0xE9, 0xFF),
            panel_bg: (0xFB, 0xFC, 0xFE, 0xFF),
            panel_edge: (0xDF, 0xE3, 0xE9, 0xFF),
            field_bg: (0xF1, 0xF3, 0xF6, 0xFF),
            button_bg: (0xE8, 0xED, 0xF5, 0xFF),
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
    fn dark_is_the_navicat_inspired_palette() {
        let dark = Theme::dark();
        assert_eq!(dark.syntax, SyntaxThemeKind::Dark);
        assert_eq!(dark.editor_bg, (0x1C, 0x20, 0x26));
        assert_eq!(dark.fg, (0xD8, 0xDC, 0xE3));
        assert_eq!(dark.dim, (0x7D, 0x85, 0x92));
        assert_eq!(dark.error, (0xE5, 0x63, 0x63), "rojo de stderr");
        assert_eq!(dark.accent, (0x5B, 0x8D, 0xEF));
        assert_eq!(dark.gutter_fg, (0x7D, 0x85, 0x92));
        assert_eq!(dark.gutter_current, (0x5B, 0x8D, 0xEF));
        assert_eq!(dark.caret, (0x5B, 0x8D, 0xEF, 0xFF));
        assert_eq!(dark.selection, (0x1F, 0x2D, 0x47, 0xFF));
        assert_eq!(dark.match_bg, (0x5B, 0x8D, 0xEF, 0x3A));
        assert_eq!(dark.current_match, (0x5B, 0x8D, 0xEF, 0x8C));
        assert_eq!(dark.bar_bg, (0x14, 0x17, 0x1C, 0xFF));
        assert_eq!(dark.tab_bg, (0x17, 0x1B, 0x21, 0xFF));
        assert_eq!(dark.tab_hover_bg, (0x1F, 0x24, 0x2C, 0xFF));
        assert_eq!(dark.tab_active_bg, (0x1C, 0x20, 0x26, 0xFF));
        assert_eq!(dark.tab_edge, (0x2C, 0x31, 0x3A, 0xFF));
        assert_eq!(dark.panel_bg, (0x19, 0x1D, 0x23, 0xFF));
        assert_eq!(dark.panel_edge, (0x2C, 0x31, 0x3A, 0xFF));
        assert_eq!(dark.field_bg, (0x14, 0x17, 0x1C, 0xFF));
        assert_eq!(dark.button_bg, (0x23, 0x29, 0x32, 0xFF));
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
    fn accent_drives_caret_gutter_and_matches() {
        assert_eq!(Theme::dark().accent, (0x5B, 0x8D, 0xEF));
        assert_eq!(Theme::light().accent, (0x2F, 0x6F, 0xED));
        for theme in [Theme::dark(), Theme::light()] {
            let (r, g, b) = theme.accent;
            assert_eq!(theme.caret, (r, g, b, 0xFF), "caret = acento");
            assert_eq!(theme.gutter_current, theme.accent, "línea actual");
            assert_eq!(
                (theme.match_bg.0, theme.match_bg.1, theme.match_bg.2),
                theme.accent
            );
            assert!(
                theme.match_bg.3 < theme.current_match.3,
                "la coincidencia actual destaca sobre las demás"
            );
        }
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
