//! Documento de texto sobre `ropey`.
//!
//! Invariantes:
//! - Internamente el texto usa siempre `\n`; [`LineEnding`] solo se aplica al
//!   guardar (y se detecta al abrir). Edición y render nunca ven `\r`.
//! - `revision` se incrementa en cada mutación para invalidar caches derivadas
//!   (layout de líneas, syntax highlighting).

use std::ops::Range;
use std::path::Path;

use ropey::Rope;

/// Fin de línea con el que el archivo se escribe en disco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

#[derive(Debug)]
pub struct Buffer {
    rope: Rope,
    revision: u64,
    line_ending: LineEnding,
    dirty: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            revision: 0,
            line_ending: LineEnding::default(),
            dirty: false,
        }
    }

    /// Crea un buffer desde texto: normaliza `\r\n`/`\r` a `\n` y detecta EOL.
    pub fn from_text(text: &str) -> Self {
        let (normalized, line_ending) = normalize(text);
        Self {
            rope: Rope::from_str(&normalized),
            revision: 0,
            line_ending,
            dirty: false,
        }
    }

    /// Abre un archivo UTF-8 detectando su fin de línea.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(Self::from_text(&raw))
    }

    /// Guarda aplicando `line_ending` y limpia el bit de sucio.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.text_with_line_ending())?;
        self.dirty = false;
        Ok(())
    }

    /// Texto completo con los `\n` convertidos a `line_ending`.
    pub fn text_with_line_ending(&self) -> String {
        let text = self.rope.to_string();
        match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }

    pub fn insert(&mut self, char_offset: usize, text: &str) {
        self.rope
            .insert(char_offset.min(self.rope.len_chars()), text);
        self.bump();
    }

    pub fn delete(&mut self, char_range: Range<usize>) {
        let start = char_range.start.min(self.rope.len_chars());
        let end = char_range.end.min(self.rope.len_chars());
        if start >= end {
            return;
        }
        self.rope.remove(start..end);
        self.bump();
    }

    fn bump(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Número de líneas (ropey: `"a\n"` → 2 líneas, la segunda vacía).
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Offset del inicio de `line`; fuera de rango → fin del documento.
    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope
            .try_line_to_char(line)
            .unwrap_or(self.rope.len_chars())
    }

    /// Línea que contiene `char_offset`; fuera de rango → última línea.
    pub fn char_to_line(&self, char_offset: usize) -> usize {
        self.rope
            .try_char_to_line(char_offset)
            .unwrap_or_else(|_| self.rope.len_lines().saturating_sub(1))
    }

    /// Texto de la línea sin su terminador.
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope
            .line(line)
            .to_string()
            .trim_end_matches('\n')
            .to_owned()
    }

    /// Extrae un rango de chars como texto.
    pub fn text_range(&self, char_range: Range<usize>) -> String {
        let start = char_range.start.min(self.rope.len_chars());
        let end = char_range.end.min(self.rope.len_chars());
        if start >= end {
            return String::new();
        }
        self.rope.slice(start..end).to_string()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }
}

/// Detecta el EOL del archivo y normaliza el texto interno a `\n`.
fn normalize(raw: &str) -> (String, LineEnding) {
    let line_ending = if raw.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            text.push('\n');
        } else {
            text.push(c);
        }
    }
    (text, line_ending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_detects_crlf_and_strips_cr() {
        let (text, eol) = normalize("a\r\nb\rc");
        assert_eq!(text, "a\nb\nc");
        assert_eq!(eol, LineEnding::Crlf);
    }

    #[test]
    fn load_and_save_roundtrip_preserves_line_ending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hola.txt");
        std::fs::write(&path, "línea1\r\nlínea2\r\n").unwrap();

        let mut buffer = Buffer::load(&path).unwrap();
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        assert_eq!(buffer.line_count(), 3);
        assert_eq!(buffer.line_text(0), "línea1");
        assert!(!buffer.dirty());

        buffer.insert(buffer.len_chars(), "x");
        assert!(buffer.dirty());
        buffer.save(&path).unwrap();
        assert!(!buffer.dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "línea1\r\nlínea2\r\nx"
        );
    }

    #[test]
    fn insert_delete_updates_text_and_revision() {
        let mut buffer = Buffer::new();
        assert_eq!(buffer.revision(), 0);

        buffer.insert(0, "hola");
        buffer.insert(4, " mundo");
        buffer.delete(4..6);
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "holaundo");
        assert!(buffer.dirty());
        assert_eq!(buffer.revision(), 3);
    }

    #[test]
    fn line_mapping_with_utf8() {
        let buffer = Buffer::from_text("áé\nño");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_to_char(1), 3);
        assert_eq!(buffer.char_to_line(2), 0); // sobre el \n
        assert_eq!(buffer.char_to_line(3), 1);
        assert_eq!(buffer.char_to_line(buffer.len_chars()), 1); // caret al final
        assert_eq!(buffer.line_text(1), "ño");
    }
}
