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

/// Edición atómica con su inversa (el material del undo/redo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Inserta `text` en el offset `at`.
    Insert { at: usize, text: String },
    /// Borra `text.chars().count()` chars a partir de `at`.
    Delete { at: usize, text: String },
}

impl Edit {
    pub fn inverse(&self) -> Edit {
        match self {
            Edit::Insert { at, text } => Edit::Delete {
                at: *at,
                text: text.clone(),
            },
            Edit::Delete { at, text } => Edit::Insert {
                at: *at,
                text: text.clone(),
            },
        }
    }

    /// Aplica el edit (sin grabar: solo graba lo hecho entre `begin`/`take_edits`).
    pub fn apply(&self, buffer: &mut Buffer) {
        match self {
            Edit::Insert { at, text } => buffer.insert(*at, text),
            Edit::Delete { at, text } => buffer.delete(*at..*at + text.chars().count()),
        }
    }
}

#[derive(Debug)]
pub struct Buffer {
    rope: Rope,
    revision: u64,
    line_ending: LineEnding,
    dirty: bool,
    /// Edits grabados entre `begin` y `take_edits` (para armar la transacción).
    journal: Option<Vec<Edit>>,
    /// Primera línea afectada por ediciones aún no consumidas (syntax cache).
    dirty_from_line: usize,
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
            journal: None,
            dirty_from_line: usize::MAX,
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
            journal: None,
            dirty_from_line: usize::MAX,
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

    /// Marca el buffer como sucio sin tocar el texto (hot exit: el contenido
    /// restaurado puede no coincidir con el disco).
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Texto completo con los `\n` convertidos a `line_ending`.
    pub fn text_with_line_ending(&self) -> String {
        let text = self.rope.to_string();
        match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }

    /// Empieza a grabar los edits (una acción de usuario = una transacción).
    pub fn begin(&mut self) {
        self.journal = Some(Vec::new());
    }

    /// Termina la grabación y devuelve los edits del bloque.
    pub fn take_edits(&mut self) -> Vec<Edit> {
        self.journal.take().unwrap_or_default()
    }

    /// Toma (y limpia) la primera línea afectada por ediciones sin consumir.
    pub fn take_dirty_from_line(&mut self) -> Option<usize> {
        (self.dirty_from_line != usize::MAX).then(|| {
            let line = self.dirty_from_line;
            self.dirty_from_line = usize::MAX;
            line
        })
    }

    fn note_dirty_at(&mut self, char_offset: usize) {
        // ANTES de mutar: el offset sigue indexando la línea vieja.
        let line = self.char_to_line(char_offset);
        if line < self.dirty_from_line {
            self.dirty_from_line = line;
        }
    }

    pub fn insert(&mut self, char_offset: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let at = char_offset.min(self.rope.len_chars());
        self.note_dirty_at(at);
        self.rope.insert(at, text);
        if let Some(journal) = &mut self.journal {
            journal.push(Edit::Insert {
                at,
                text: text.to_owned(),
            });
        }
        self.bump();
    }

    pub fn delete(&mut self, char_range: Range<usize>) {
        let start = char_range.start.min(self.rope.len_chars());
        let end = char_range.end.min(self.rope.len_chars());
        if start >= end {
            return;
        }
        self.note_dirty_at(start);
        let text = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        if let Some(journal) = &mut self.journal {
            journal.push(Edit::Delete { at: start, text });
        }
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

    #[test]
    fn journal_records_edits_for_undo() {
        let mut buffer = Buffer::from_text("hola");
        buffer.begin();
        buffer.delete(1..3); // borra "ol" → "ha"
        buffer.insert(1, "XX");
        let edits = buffer.take_edits();
        assert_eq!(
            edits,
            vec![
                Edit::Delete {
                    at: 1,
                    text: "ol".to_owned()
                },
                Edit::Insert {
                    at: 1,
                    text: "XX".to_owned()
                },
            ]
        );
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "hXXa");

        // Sin grabación activa no se registran edits.
        buffer.insert(0, "!");
        assert!(buffer.take_edits().is_empty());

        // El inverse restaura el texto original.
        let mut restored = Buffer::from_text("hola");
        for edit in edits.iter().rev().map(Edit::inverse) {
            edit.apply(&mut restored);
        }
        assert_eq!(restored.text_range(0..restored.len_chars()), "hola");
    }

    #[test]
    fn dirty_from_line_reports_lowest_edited_line() {
        let mut buffer = Buffer::from_text("uno\ndos\ntres");
        assert_eq!(buffer.take_dirty_from_line(), None);

        buffer.insert(0, "x");
        assert_eq!(buffer.take_dirty_from_line(), Some(0));
        assert_eq!(buffer.take_dirty_from_line(), None, "se limpia al tomar");

        // Edición en la línea 2 + otra en la línea 1 → reporta la menor.
        buffer.insert(buffer.len_chars(), "!");
        buffer.insert(5, "y"); // línea 1
        assert_eq!(buffer.take_dirty_from_line(), Some(1));
    }

    #[test]
    fn set_dirty_marks_without_touching_text() {
        let mut buffer = Buffer::from_text("calmado");
        assert!(!buffer.dirty());
        let revision = buffer.revision();

        buffer.set_dirty();
        assert!(buffer.dirty(), "hot exit marca sucio sin editar");
        assert_eq!(buffer.revision(), revision, "y no altera la revisión");
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "calmado");
    }
}
