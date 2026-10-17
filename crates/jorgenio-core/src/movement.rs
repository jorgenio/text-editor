//! Movimiento del caret (grafemas Unicode) y mapeos de columna visual.
//!
//! Las posiciones son offsets de **char** (nativos de ropey). La columna
//! visual cuenta **celdas**: tabs hasta el siguiente tab stop y caracteres
//! anchos (CJK/emoji) como 2 celdas.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::buffer::Buffer;

/// Offset del grafo anterior. Desde el inicio de una línea regresa al final
/// de la anterior (posición sobre su `\n`).
pub fn prev_grapheme(buffer: &Buffer, caret: usize) -> usize {
    if caret == 0 {
        return 0;
    }
    let line = buffer.char_to_line(caret);
    let line_start = buffer.line_to_char(line);
    if caret <= line_start {
        return line_start - 1;
    }
    let text = buffer.text_range(line_start..caret);
    let last = text
        .grapheme_indices(true)
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0);
    line_start + text[..last].chars().count()
}

/// Offset del grafo siguiente. Desde el final de línea salta el `\n`.
pub fn next_grapheme(buffer: &Buffer, caret: usize) -> usize {
    if caret >= buffer.len_chars() {
        return buffer.len_chars();
    }
    let line = buffer.char_to_line(caret);
    let line_end = buffer.line_to_char(line + 1);
    let text = buffer.text_range(caret..line_end);
    let next = text
        .grapheme_indices(true)
        .nth(1)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    caret + text[..next].chars().count()
}

pub fn line_start(buffer: &Buffer, caret: usize) -> usize {
    buffer.line_to_char(buffer.char_to_line(caret))
}

/// Fin de línea: posición sobre el `\n`, o al final si no hay terminador.
pub fn line_end(buffer: &Buffer, caret: usize) -> usize {
    let line = buffer.char_to_line(caret);
    if line + 1 >= buffer.line_count() {
        buffer.line_to_char(line + 1)
    } else {
        buffer.line_to_char(line + 1) - 1
    }
}

/// Mueve el caret `delta` líneas manteniendo la columna visual.
pub fn vertical_move(buffer: &Buffer, caret: usize, delta: isize, tab_width: usize) -> usize {
    let line_count = buffer.line_count();
    let line = buffer.char_to_line(caret);
    let target = (line as isize + delta).clamp(0, line_count as isize - 1) as usize;
    if target == line {
        return caret;
    }
    let line_text = buffer.line_text(line);
    let char_col = caret.saturating_sub(buffer.line_to_char(line));
    let visual = visual_column(&line_text, char_col, tab_width);

    let target_text = buffer.line_text(target);
    let target_col = char_col_at_visual(&target_text, visual, tab_width);
    buffer.line_to_char(target) + target_col
}

/// Columna visual (celdas) de `char_col` dentro de `line` (sin terminador).
pub fn visual_column(line: &str, char_col: usize, tab_width: usize) -> usize {
    let target = byte_at_char(line, char_col);
    let mut column = 0;
    for (i, grapheme) in line.grapheme_indices(true) {
        if i >= target {
            break;
        }
        column += grapheme_cells(grapheme, column, tab_width);
    }
    column
}

/// Columnas visuales de varios offsets **byte** de `line` en UN solo recorrido
/// de grafemas. Equivalente a llamar [`visual_column`] con cada offset (los
/// offsets deben estar ordenados y ser límites de char), pero O(línea) en
/// total: indispensable al resaltar, donde cada token pide su columna.
pub fn visual_columns_at(line: &str, targets: &[usize], tab_width: usize) -> Vec<usize> {
    let mut columns = Vec::with_capacity(targets.len());
    let mut pending = 0;
    let mut column = 0;
    for (i, grapheme) in line.grapheme_indices(true) {
        while pending < targets.len() && targets[pending] <= i {
            columns.push(column);
            pending += 1;
        }
        column += grapheme_cells(grapheme, column, tab_width);
    }
    while pending < targets.len() {
        columns.push(column);
        pending += 1;
    }
    columns
}

/// Char (unidad de char, no byte) para una celda visual; regla de media celda.
pub fn char_col_at_visual(line: &str, visual_col: usize, tab_width: usize) -> usize {
    let mut column = 0;
    for (i, grapheme) in line.grapheme_indices(true) {
        let cells = grapheme_cells(grapheme, column, tab_width);
        if visual_col < column + cells {
            let before = visual_col - column < cells.div_ceil(2);
            let byte = if before { i } else { i + grapheme.len() };
            return line[..byte].chars().count();
        }
        column += cells;
    }
    line.chars().count()
}

fn grapheme_cells(grapheme: &str, column: usize, tab_width: usize) -> usize {
    if grapheme == "\t" {
        tab_width - (column % tab_width.max(1))
    } else {
        grapheme.width().max(1)
    }
}

pub fn byte_at_char(line: &str, char_col: usize) -> usize {
    line.char_indices()
        .nth(char_col)
        .map_or(line.len(), |(i, _)| i)
}

/// Rango de la palabra bajo `pos` (doble click): letras/dígitos/`_` se toman
/// como una unidad; los espacios forman su propio run; nunca cruza líneas.
pub fn word_range(buffer: &Buffer, pos: usize) -> std::ops::Range<usize> {
    use std::ops::Range;

    let len = buffer.len_chars();
    if len == 0 {
        return Range { start: 0, end: 0 };
    }
    let pos = pos.min(len);
    let line = buffer.char_to_line(pos);
    let line_start = buffer.line_to_char(line);
    let text = buffer.line_text(line);
    let chars: Vec<char> = text.chars().collect();

    // Clic justo tras una palabra (el char es el espacio siguiente) → esa palabra.
    let mut col = pos.saturating_sub(line_start).min(chars.len());
    if col == chars.len() {
        col = chars.len().saturating_sub(1);
    } else if !is_word_char(chars[col]) && col > 0 && is_word_char(chars[col - 1]) {
        col -= 1;
    }
    if chars.is_empty() {
        // Línea vacía: selecciona su terminador (\n) si existe.
        return Range {
            start: line_start,
            end: (line_start + 1).min(len),
        };
    }

    let mut begin = col;
    let mut end = col + 1;
    if is_word_char(chars[col]) {
        while begin > 0 && is_word_char(chars[begin - 1]) {
            begin -= 1;
        }
        while end < chars.len() && is_word_char(chars[end]) {
            end += 1;
        }
    } else if chars[col].is_whitespace() {
        while begin > 0 && chars[begin - 1].is_whitespace() {
            begin -= 1;
        }
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
    }

    Range {
        start: line_start + begin,
        end: line_start + end,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Inicio de la palabra anterior (Ctrl+←): salta espacios hacia atrás y
/// luego el run entero (palabra o puntuación). Cruza líneas; en el inicio
/// del buffer devuelve 0.
pub fn word_left(buffer: &Buffer, pos: usize) -> usize {
    let len = buffer.len_chars();
    let mut pos = pos.min(len);
    if pos == 0 {
        return 0;
    }
    loop {
        let line = buffer.char_to_line(pos);
        let line_start = buffer.line_to_char(line);
        let chars: Vec<char> = buffer.line_text(line).chars().collect();
        let mut col = (pos - line_start).min(chars.len());

        // 1) Espacios hacia atrás (el \n se resuelve al bajar de línea).
        while col > 0 && chars[col - 1].is_whitespace() {
            col -= 1;
        }
        // 2) Run actual (palabra o puntuación) hacia atrás → su inicio.
        if col > 0 {
            let word = is_word_char(chars[col - 1]);
            while col > 0 && !chars[col - 1].is_whitespace() && is_word_char(chars[col - 1]) == word
            {
                col -= 1;
            }
            return line_start + col;
        }
        // 3) Inicio de línea: el \n de la anterior es un espacio más.
        if line == 0 {
            return 0;
        }
        pos = line_start - 1;
    }
}

/// Inicio de la palabra siguiente (Ctrl+→): consume el run actual, salta
/// los espacios (y líneas) y para en el primer char no-blanco.
pub fn word_right(buffer: &Buffer, pos: usize) -> usize {
    let len = buffer.len_chars();
    let mut pos = pos.min(len);
    if pos >= len {
        return len;
    }
    loop {
        let line = buffer.char_to_line(pos);
        let line_start = buffer.line_to_char(line);
        let chars: Vec<char> = buffer.line_text(line).chars().collect();
        let mut col = (pos - line_start).min(chars.len());

        // 1) Run actual (palabra o puntuación) hacia delante.
        if col < chars.len() && !chars[col].is_whitespace() {
            let word = is_word_char(chars[col]);
            while col < chars.len()
                && !chars[col].is_whitespace()
                && is_word_char(chars[col]) == word
            {
                col += 1;
            }
        }
        // 2) Espacios hacia delante dentro de la línea.
        while col < chars.len() && chars[col].is_whitespace() {
            col += 1;
        }
        if col < chars.len() {
            return line_start + col;
        }
        // 3) Fin de línea: el \n es espacio; se pasa a la siguiente.
        let next = line_start + chars.len() + 1;
        if next >= len {
            return len;
        }
        pos = next;
        // Tras cruzar, si la línea empieza por run no-ws, AHÍ se para
        // (una sola pulsación no salta dos palabras).
        let next_line = buffer.char_to_line(pos);
        let next_start = buffer.line_to_char(next_line);
        let next_chars: Vec<char> = buffer.line_text(next_line).chars().collect();
        let next_col = pos - next_start;
        if next_col < next_chars.len() && !next_chars[next_col].is_whitespace() {
            return pos;
        }
        // Si empieza por espacios (o está vacía), el bucle los salta.
    }
}

/// Rango de la línea completa (incluye su `\n`; la última usa el fin del buffer).
pub fn line_range(buffer: &Buffer, line: usize) -> std::ops::Range<usize> {
    let start = buffer.line_to_char(line);
    let end = buffer.line_to_char(line + 1);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_motion_handles_emoji_and_lines() {
        let buffer = Buffer::from_text("👍🏽a\nxy");
        assert_eq!(next_grapheme(&buffer, 0), 2); // 👍🏽 ocupa 2 chars
        assert_eq!(prev_grapheme(&buffer, 2), 0);
        assert_eq!(next_grapheme(&buffer, 2), 3); // al final (sobre el \n)
        assert_eq!(next_grapheme(&buffer, 3), 4); // salta el \n
        assert_eq!(line_start(&buffer, 4), 4);
        assert_eq!(prev_grapheme(&buffer, 4), 3); // fin de la línea anterior
        assert_eq!(line_end(&buffer, 0), 3);
    }

    #[test]
    fn vertical_move_keeps_visual_column() {
        let buffer = Buffer::from_text("a\txy\n你好\nabcd");
        // Tras "a\t" → celda 4 (tab hasta el stop 4)
        assert_eq!(visual_column(&buffer.line_text(0), 2, 4), 4);
        let caret = vertical_move(&buffer, 2, 1, 4);
        // "你好" es 4 celdas: celda 4 = tras ambos
        assert_eq!(caret, buffer.line_to_char(1) + 2);
        // Volver arriba conserva la celda 4 = justo tras la tab (char 2)
        let back = vertical_move(&buffer, caret, -1, 4);
        assert_eq!(back, buffer.line_to_char(0) + 2);
    }

    #[test]
    fn click_mapping_uses_half_cell_rule() {
        assert_eq!(char_col_at_visual("a\tb", 0, 4), 0);
        assert_eq!(char_col_at_visual("a\tb", 2, 4), 1); // mitad izq del tab → antes
        assert_eq!(char_col_at_visual("a\tb", 3, 4), 2); // mitad der → después
        assert_eq!(char_col_at_visual("a\tb", 99, 4), 3); // más allá del final

        assert_eq!(visual_column("你好", 1, 4), 2);
        assert_eq!(char_col_at_visual("你好", 0, 4), 0);
        assert_eq!(char_col_at_visual("你好", 1, 4), 1); // mitad der del 1er char
        assert_eq!(char_col_at_visual("你好", 2, 4), 1); // mitad izq del 2º
    }

    #[test]
    fn word_range_selects_words_spaces_and_stays_on_line() {
        let buffer = Buffer::from_text("hola mundo\nfoo_bar 42");
        assert_eq!(word_range(&buffer, 0), 0..4);
        assert_eq!(word_range(&buffer, 2), 0..4);
        assert_eq!(word_range(&buffer, 5), 5..10);
        // Clic en el espacio tras "hola" (col 4) → la palabra anterior.
        assert_eq!(word_range(&buffer, 4), 0..4);
        // Clic al final de la primera línea (pos 10 = sobre el \n).
        assert_eq!(word_range(&buffer, 10), 5..10);
        // Underscore es parte de la palabra; segunda línea con línea propia.
        assert_eq!(word_range(&buffer, 12), 11..18); // "foo_bar"
        assert_eq!(word_range(&buffer, 19), 19..21); // "42"
        // Clic justo tras "foo_bar" → la palabra anterior (regla de borde).
        assert_eq!(word_range(&buffer, 18), 11..18);

        // Un run de espacios varios sí se selecciona como tal.
        let spaces = Buffer::from_text("ab   cd");
        assert_eq!(word_range(&spaces, 4), 2..5);
        assert_eq!(word_range(&spaces, 2), 0..2); // tras "ab" → la palabra
    }

    #[test]
    fn word_range_on_empty_line_selects_its_newline() {
        let buffer = Buffer::from_text("abc\n");
        // pos 4 = línea vacía final: selecciona el \n inexistente → fin del buffer.
        assert_eq!(word_range(&buffer, 4), 4..4);
        let buffer = Buffer::from_text("abc\n\nx");
        assert_eq!(word_range(&buffer, 4), 4..5); // la \n de la línea vacía
    }

    #[test]
    fn line_range_includes_newline() {
        let buffer = Buffer::from_text("uno\ndos\n");
        assert_eq!(line_range(&buffer, 0), 0..4);
        assert_eq!(line_range(&buffer, 1), 4..8);
        assert_eq!(line_range(&buffer, 2), 8..8); // línea vacía final
    }

    #[test]
    fn visual_columns_at_matches_visual_column_at_each_boundary() {
        // Tabs + CJK + cluster emoji (fronteras de char dentro del grafo).
        let line = "a\tbb\tc東家ñ\t👍🏽z";
        let tab = 4;
        let mut targets: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
        targets.push(line.len());

        let batch = visual_columns_at(line, &targets, tab);
        assert_eq!(batch.len(), targets.len());
        for (i, &target) in targets.iter().enumerate() {
            let char_col = line[..target].chars().count();
            assert_eq!(
                batch[i],
                visual_column(line, char_col, tab),
                "distinto en byte {target}"
            );
        }
    }

    #[test]
    fn visual_columns_at_handles_empty_line_and_no_targets() {
        assert_eq!(visual_columns_at("", &[0, 3], 4), vec![0, 0]);
        assert!(visual_columns_at("hola", &[], 4).is_empty());
    }

    #[test]
    fn word_motion_crosses_words_spaces_and_lines() {
        // 0123456 7 89 0123456 → "foo bar\n  baz(qux" (len 17)
        let buffer = Buffer::from_text("foo bar\n  baz(qux");
        // word_right: run + espacios → inicio de la siguiente run.
        assert_eq!(word_right(&buffer, 0), 4, "foo → bar");
        assert_eq!(word_right(&buffer, 3), 4, "del espacio salta a bar");
        assert_eq!(word_right(&buffer, 4), 10, "bar → cruza el \\n → baz");
        assert_eq!(word_right(&buffer, 7), 10, "desde el \\n → baz");
        assert_eq!(word_right(&buffer, 8), 10, "los espacios se saltan");
        assert_eq!(word_right(&buffer, 10), 13, "baz → el paréntesis");
        assert_eq!(word_right(&buffer, 13), 14, "( → qux");
        assert_eq!(
            word_right(&buffer, 14),
            buffer.len_chars(),
            "al final → len"
        );

        // word_left: espacios y luego el run entero hacia su inicio.
        assert_eq!(word_left(&buffer, 4), 0, "bar → foo");
        assert_eq!(word_left(&buffer, 2), 0, "en medio de foo → su inicio");
        assert_eq!(word_left(&buffer, 10), 4, "baz → inicio de bar");
        assert_eq!(word_left(&buffer, 7), 4, "desde el \\n → bar");
        let buffer2 = Buffer::from_text("ab cd");
        assert_eq!(word_left(&buffer2, 3), 0, "inicio de cd → inicio de ab");
        assert_eq!(word_left(&buffer2, 0), 0, "ya en el inicio");
        assert_eq!(word_right(&buffer2, 5), 5, "ya en el final");

        // Línea vacía en medio: el salto la atraviesa.
        let gaps = Buffer::from_text("a\n\nb");
        assert_eq!(word_right(&gaps, 1), 3, "a → atraviesa la línea vacía → b");
        assert_eq!(word_left(&gaps, 3), 0, "b → a");
    }
}
