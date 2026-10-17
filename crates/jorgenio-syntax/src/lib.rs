//! Syntax highlighting de Jorgenio Text.
//!
//! Envuelve `syntect` (gramáticas `.sublime-syntax` y temas `.tmTheme`
//! originales de Sublime Text) con una cache incremental:
//!
//! - `states[i]` = estado del parser **al inicio** de la línea `i`.
//! - `tokens[i]`  = piezas coloreadas de la línea `i` (válido hasta que el
//!   buffer reporta una edición desde esa línea: [`Buffer::take_dirty_from_line`]).
//!
//! Solo se resaltan las líneas pedidas ([`SyntaxHighlighter::prepare`]), que es
//! hasta la última visible: tipear en un archivo de 100k líneas re-resalta
//! únicamente desde la línea editada hasta el final de la pantalla.

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

use jorgenio_core::buffer::Buffer;
use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Tema Monokai (paleta clásica de Sublime Text), escrito para este proyecto.
const MONOKAI: &str = include_str!("../../../assets/themes/Monokai.tmTheme");

/// Paleta de syntect: Monokai (oscuro, por defecto) o InspiredGitHub (claro,
/// para fondo blanco). El cambio invalida los tokens cacheados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxThemeKind {
    Dark,
    Light,
}

/// Una pieza coloreada de una línea, en **bytes** dentro de la línea sin EOL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub color: (u8, u8, u8),
}

#[derive(Clone)]
struct Checkpoint {
    parse: ParseState,
    highlight: HighlightState,
}

pub struct SyntaxHighlighter {
    /// Extensión actual ("" = sin gramática → texto plano).
    extension: String,
    plain: bool,
    /// Paleta de colores activa (oscuro/claro).
    kind: SyntaxThemeKind,
    /// Estado del parser al inicio de cada línea (states[0] = inicial).
    states: Vec<Checkpoint>,
    /// Tokens por línea; `tokens.len()` = cuántas líneas están resaltadas.
    tokens: Vec<Vec<Token>>,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        let mut highlighter = Self {
            extension: String::new(),
            plain: true,
            kind: SyntaxThemeKind::Dark,
            states: Vec::new(),
            tokens: Vec::new(),
        };
        highlighter.reset();
        highlighter
    }

    /// Extensión de archivo como clave de syntect ("rs", "py"…); "" sin extensión.
    pub fn detect(path: &Path) -> String {
        path.extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// Cambia el lenguaje por extensión; invalida todo si realmente cambió.
    pub fn set_extension(&mut self, extension: &str) {
        if self.extension == extension {
            return;
        }
        self.extension = extension.to_owned();
        self.reset();
    }

    pub fn extension(&self) -> &str {
        &self.extension
    }

    /// Cambia la paleta (oscuro/claro); invalida todo si realmente cambió.
    pub fn set_theme(&mut self, kind: SyntaxThemeKind) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        self.reset();
    }

    /// Invalida los tokens desde `line` (edición reportada por el buffer).
    pub fn invalidate(&mut self, line: usize) {
        let keep = line.min(self.tokens.len());
        self.tokens.truncate(keep);
        // states[0..=line] siguen válidos: el estado al inicio de una línea
        // solo depende de las líneas anteriores.
        if self.states.len() > line + 1 {
            self.states.truncate(line + 1);
        }
    }

    /// Resalta (o usa de cache) hasta la línea `last_line` inclusive.
    /// Consume la marca de suciedad del buffer.
    pub fn prepare(&mut self, buffer: &mut Buffer, last_line: usize) {
        if let Some(dirty) = buffer.take_dirty_from_line() {
            self.invalidate(dirty);
        }
        if self.plain {
            return;
        }
        for line in self.tokens.len()..=last_line {
            self.highlight_line(buffer, line);
        }
    }

    /// Tokens de la línea (vacío = sin resaltar / texto plano).
    pub fn tokens(&self, line: usize) -> &[Token] {
        self.tokens.get(line).map_or(&[], Vec::as_slice)
    }

    fn reset(&mut self) {
        let syntax = Self::syntax_for(&self.extension);
        self.plain = syntax.is_none();
        let syntax = syntax.unwrap_or_else(|| syntax_set().find_syntax_plain_text());
        self.states = vec![base_checkpoint(syntax, self.kind)];
        self.tokens.clear();
    }

    fn syntax_for(extension: &str) -> Option<&'static SyntaxReference> {
        if extension.is_empty() {
            return None;
        }
        syntax_set().find_syntax_by_extension(extension)
    }

    fn highlight_line(&mut self, buffer: &Buffer, line: usize) {
        let Some(syntax) = Self::syntax_for(&self.extension) else {
            self.tokens.push(Vec::new());
            return;
        };
        let raw = buffer.line_text(line);
        let text = if line + 1 < buffer.line_count() {
            Cow::Owned(format!("{raw}\n"))
        } else {
            Cow::Borrowed(raw.as_str())
        };

        let base = self
            .states
            .get(line)
            .cloned()
            .unwrap_or_else(|| base_checkpoint(syntax, self.kind));
        let (next, tokens) = self.tokenize(base, &text);
        self.tokens.push(tokens);

        if self.states.len() == line + 1 {
            self.states.push(next);
        } else if self.states.len() > line + 1 {
            self.states[line + 1] = next;
        } else {
            // No debería pasar (invariante states.len() ≥ tokens.len()+1).
            while self.states.len() <= line {
                self.states.push(base_checkpoint(syntax, self.kind));
            }
            self.states.push(next);
        }
    }

    /// Resalta un texto (con su `\n` si aplica) y devuelve (estado siguiente, tokens).
    fn tokenize(&self, mut checkpoint: Checkpoint, text: &str) -> (Checkpoint, Vec<Token>) {
        let highlighter = Highlighter::new(theme_for(self.kind));
        let Ok(ops) = checkpoint.parse.parse_line(text, syntax_set()) else {
            return (checkpoint, Vec::new());
        };

        let mut tokens = Vec::new();
        let mut offset = 0;
        for (style, piece) in
            HighlightIterator::new(&mut checkpoint.highlight, &ops, text, &highlighter)
        {
            let start = offset;
            let end = offset + piece.len();
            offset = end;
            // Syntect incluye el `\n` final en la última pieza (modo newlines).
            if start >= raw_len(text) {
                continue;
            }
            tokens.push(Token {
                start,
                end: end.min(raw_len(text)),
                color: (style.foreground.r, style.foreground.g, style.foreground.b),
            });
        }
        (checkpoint, tokens)
    }
}

/// Largo del texto SIN contar un `\n` final (syntect resalta la línea con EOL).
fn raw_len(text_with_nl: &str) -> usize {
    if text_with_nl.ends_with('\n') {
        text_with_nl.len() - 1
    } else {
        text_with_nl.len()
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_for(kind: SyntaxThemeKind) -> &'static Theme {
    match kind {
        SyntaxThemeKind::Dark => dark_theme(),
        SyntaxThemeKind::Light => light_theme(),
    }
}

fn dark_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        ThemeSet::load_from_reader(&mut std::io::Cursor::new(MONOKAI))
            .expect("assets/themes/Monokai.tmTheme es un .tmTheme válido")
    })
}

/// Paleta clara incluida en syntect (diseñada para fondo blanco).
fn light_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        ThemeSet::load_defaults()
            .themes
            .get("InspiredGitHub")
            .cloned()
            .expect("syntect incluye el tema InspiredGitHub")
    })
}

fn base_checkpoint(syntax: &SyntaxReference, kind: SyntaxThemeKind) -> Checkpoint {
    let highlighter = Highlighter::new(theme_for(kind));
    Checkpoint {
        parse: ParseState::new(syntax),
        highlight: HighlightState::new(&highlighter, ScopeStack::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(tokens: &[Token]) -> Vec<(u8, u8, u8)> {
        tokens.iter().map(|token| token.color).collect()
    }

    #[test]
    fn detect_maps_extensions() {
        assert_eq!(SyntaxHighlighter::detect(Path::new("src/main.rs")), "rs");
        assert_eq!(SyntaxHighlighter::detect(Path::new("A.TXT")), "txt");
        assert_eq!(SyntaxHighlighter::detect(Path::new("Makefile")), "");
    }

    #[test]
    fn rust_keywords_get_monokai_pink() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("fn main() {\n    let x = 42;\n}");
        syntax.prepare(&mut buffer, 2);

        let first_line = colors(syntax.tokens(0));
        assert!(!first_line.is_empty());
        assert!(
            first_line.contains(&(0xF9, 0x26, 0x72)),
            "`fn` debe ser rosa Monokai: {first_line:?}"
        );
        // La constante numérica sale violeta.
        let second_line = colors(syntax.tokens(1));
        assert!(
            second_line.contains(&(0xAE, 0x81, 0xBE)),
            "`42` debe ser violeta: {second_line:?}"
        );
    }

    #[test]
    fn block_comment_state_spans_lines() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("/* inicio\nmitad\nfin */ let x = 1;");
        syntax.prepare(&mut buffer, 2);

        let gray = (0x75, 0x71, 0x5E);
        assert!(
            colors(syntax.tokens(1)).contains(&gray),
            "línea 2 en comentario"
        );
        assert!(
            colors(syntax.tokens(2)).contains(&gray),
            "apertura en línea 3"
        );
        // Tras el cierre, `let` vuelve a rosa en la misma línea.
        assert!(
            colors(syntax.tokens(2)).contains(&(0xF9, 0x26, 0x72)),
            "`let` tras `*/` debe ser rosa"
        );
    }

    #[test]
    fn edit_invalidates_from_changed_line_only() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("let a = 1;\nlet b = 2;");
        syntax.prepare(&mut buffer, 1);
        assert!(!syntax.tokens(1).is_empty());

        buffer.insert(0, "// ");
        syntax.prepare(&mut buffer, 1);
        // Línea 0 ahora es un comentario completo: gris.
        assert!(colors(syntax.tokens(0)).contains(&(0x75, 0x71, 0x5E)));
        // La línea 1 sigue resaltada (se re-parsea pero conserva keywords).
        assert!(colors(syntax.tokens(1)).contains(&(0xF9, 0x26, 0x72)));
    }

    #[test]
    fn plain_extension_yields_no_tokens() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("jorgenio-unknown");
        let mut buffer = Buffer::from_text("fn main() {}");
        syntax.prepare(&mut buffer, 0);
        assert!(syntax.tokens(0).is_empty());
    }

    #[test]
    fn prepare_is_idempotent() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("fn foo() {}");
        syntax.prepare(&mut buffer, 0);
        let first = colors(syntax.tokens(0));
        syntax.prepare(&mut buffer, 0);
        assert_eq!(first, colors(syntax.tokens(0)));
    }

    #[test]
    fn set_theme_light_recolors_and_dark_restores() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("fn main() {}");
        syntax.prepare(&mut buffer, 0);
        let dark_colors = colors(syntax.tokens(0));
        assert!(
            dark_colors.contains(&(0xF9, 0x26, 0x72)),
            "Monokai por defecto"
        );

        syntax.set_theme(SyntaxThemeKind::Light);
        assert!(syntax.tokens(0).is_empty(), "el cambio invalida la cache");
        syntax.prepare(&mut buffer, 0);
        let light_colors = colors(syntax.tokens(0));
        assert!(!light_colors.is_empty(), "la paleta clara resalta");
        assert!(
            !light_colors.contains(&(0xF9, 0x26, 0x72)),
            "InspiredGitHub no usa el rosa Monokai: {light_colors:?}"
        );

        syntax.set_theme(SyntaxThemeKind::Dark);
        syntax.prepare(&mut buffer, 0);
        assert_eq!(colors(syntax.tokens(0)), dark_colors, "vuelve al Monokai");
    }

    #[test]
    fn set_theme_same_kind_keeps_cache() {
        let mut syntax = SyntaxHighlighter::new();
        syntax.set_extension("rs");
        let mut buffer = Buffer::from_text("let x = 1;");
        syntax.prepare(&mut buffer, 0);
        let cached = colors(syntax.tokens(0));

        syntax.set_theme(SyntaxThemeKind::Dark);
        assert_eq!(colors(syntax.tokens(0)), cached, "sin reset inútil");
    }
}
