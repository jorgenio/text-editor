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

fn byte_at_char(line: &str, char_col: usize) -> usize {
    line.char_indices()
        .nth(char_col)
        .map_or(line.len(), |(i, _)| i)
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
}
