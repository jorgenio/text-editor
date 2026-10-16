//! Historial undo/redo transaccional.
//!
//! Cada acción de edición genera una [`Transaction`] con sus edits inversos y
//! los carets (pares `caret, anchor` de cada cursor) antes/después. Las
//! pulsaciones contiguas de tecleo o backspace se fusionan (coalesce) para que
//! Ctrl+Z deshaga bloques naturales en lugar de un carácter a la vez.

use crate::buffer::{Buffer, Edit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub edits: Vec<Edit>,
    /// `(caret, anchor)` de cada cursor antes del cambio.
    pub cursors_before: Vec<(usize, usize)>,
    /// `(caret, anchor)` de cada cursor después del cambio.
    pub cursors_after: Vec<(usize, usize)>,
}

impl Transaction {
    pub fn new(edits: Vec<Edit>, before: Vec<(usize, usize)>, after: Vec<(usize, usize)>) -> Self {
        Self {
            edits,
            cursors_before: before,
            cursors_after: after,
        }
    }

    /// Transacción inversa (aplicarla deshace el cambio).
    pub fn inverse(&self) -> Self {
        Self {
            edits: self.edits.iter().rev().map(Edit::inverse).collect(),
            cursors_before: self.cursors_after.clone(),
            cursors_after: self.cursors_before.clone(),
        }
    }

    pub fn undo_position(&self) -> Vec<(usize, usize)> {
        self.cursors_before.clone()
    }

    pub fn redo_position(&self) -> Vec<(usize, usize)> {
        self.cursors_after.clone()
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra una transacción: limpia el redo y fusiona tecleo contiguo.
    pub fn commit(&mut self, txn: Transaction) {
        if txn.edits.is_empty() {
            return;
        }
        self.redo_stack.clear();
        if let Some(prev) = self.undo_stack.last_mut()
            && merge(prev, &txn)
        {
            prev.cursors_after = txn.cursors_after;
            return;
        }
        self.undo_stack.push(txn);
    }

    /// Aplica el inverse del último txn; devuelve los carets a restaurar.
    pub fn undo(&mut self, buffer: &mut Buffer) -> Option<Vec<(usize, usize)>> {
        let txn = self.undo_stack.pop()?;
        for edit in txn.inverse().edits {
            edit.apply(buffer);
        }
        let position = txn.undo_position();
        self.redo_stack.push(txn);
        Some(position)
    }

    /// Reaplica el último txn deshecho; devuelve los carets a restaurar.
    pub fn redo(&mut self, buffer: &mut Buffer) -> Option<Vec<(usize, usize)>> {
        let txn = self.redo_stack.pop()?;
        for edit in &txn.edits {
            edit.apply(buffer);
        }
        let position = txn.redo_position();
        self.undo_stack.push(txn);
        Some(position)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

/// Fusiona tecleo/backspace contiguos en el mismo bloque de undo.
fn merge(prev: &mut Transaction, new: &Transaction) -> bool {
    if prev.edits.len() != 1 || new.edits.len() != 1 {
        return false;
    }
    match (&prev.edits[0], &new.edits[0]) {
        // Escritura carácter a carácter contigua → un solo undo.
        (
            Edit::Insert {
                at: prev_at,
                text: prev_text,
            },
            Edit::Insert { at, text },
        ) => {
            let contiguous = *at == *prev_at + prev_text.chars().count();
            if !prev_text.contains('\n')
                && text.chars().count() == 1
                && !text.contains('\n')
                && contiguous
            {
                prev.edits[0] = Edit::Insert {
                    at: *prev_at,
                    text: format!("{prev_text}{text}"),
                };
                return true;
            }
            false
        }
        // Borrado contiguo: backspace (hacia la izquierda) o Delete (hacia la derecha).
        (
            Edit::Delete {
                at: prev_at,
                text: prev_text,
            },
            Edit::Delete { at, text },
        ) => {
            let backward = *at + text.chars().count() == *prev_at;
            let forward = *at == *prev_at;
            if backward {
                prev.edits[0] = Edit::Delete {
                    at: *at,
                    text: format!("{text}{prev_text}"),
                };
                return true;
            }
            if forward {
                prev.edits[0] = Edit::Delete {
                    at: *prev_at,
                    text: format!("{prev_text}{text}"),
                };
                return true;
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn_insert(at: usize, text: &str) -> Transaction {
        Transaction::new(
            vec![Edit::Insert {
                at,
                text: text.to_owned(),
            }],
            vec![(at, at)],
            vec![(at + text.chars().count(), at + text.chars().count())],
        )
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut buffer = Buffer::new();
        let mut history = History::new();

        let txn = txn_insert(0, "hola");
        buffer.insert(0, "hola");
        history.commit(txn);

        let position = history.undo(&mut buffer).expect("debe poder deshacer");
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "");
        assert_eq!(position, vec![(0, 0)]);

        let position = history.redo(&mut buffer).expect("debe poder rehacer");
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "hola");
        assert_eq!(position, vec![(4, 4)]);
    }

    #[test]
    fn delete_inverse_restores_text() {
        let mut buffer = Buffer::from_text("abcdef");
        let mut history = History::new();

        let txn = Transaction::new(
            vec![Edit::Delete {
                at: 2,
                text: "cd".to_owned(),
            }],
            vec![(4, 4)],
            vec![(2, 2)],
        );
        buffer.delete(2..4);
        history.commit(txn);

        history.undo(&mut buffer);
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "abcdef");
    }

    #[test]
    fn typing_runs_merge_and_redo_clears() {
        let mut history = History::new();
        history.commit(txn_insert(0, "a"));
        history.commit(txn_insert(1, "b"));
        history.commit(txn_insert(2, "c"));
        assert_eq!(history.undo_stack.len(), 1, "el tecleo contiguo se fusiona");

        history.commit(txn_insert(3, "d"));
        assert!(history.redo_stack.is_empty(), "commit nuevo limpia redo");

        // No fusiona si salta posición: "abc" + insert en 9.
        history.commit(txn_insert(9, "z"));
        assert_eq!(history.undo_stack.len(), 2);
    }

    #[test]
    fn backspace_runs_merge_backwards() {
        let mut history = History::new();
        history.commit(Transaction::new(
            vec![Edit::Delete {
                at: 3,
                text: "a".to_owned(),
            }],
            vec![(4, 4)],
            vec![(3, 3)],
        ));
        history.commit(Transaction::new(
            vec![Edit::Delete {
                at: 2,
                text: "l".to_owned(),
            }],
            vec![(3, 3)],
            vec![(2, 2)],
        ));
        assert_eq!(history.undo_stack.len(), 1, "backspace contiguo se fusiona");
        assert_eq!(
            history.undo_stack[0].edits[0],
            Edit::Delete {
                at: 2,
                text: "la".to_owned()
            }
        );
    }

    #[test]
    fn newline_insert_breaks_the_run() {
        let mut history = History::new();
        history.commit(txn_insert(0, "a"));
        history.commit(txn_insert(1, "\n"));
        history.commit(txn_insert(2, "b"));
        assert_eq!(history.undo_stack.len(), 3, "Enter corta el grupo de undo");
    }

    #[test]
    fn multi_cursor_positions_survive_undo() {
        let mut buffer = Buffer::new();
        let mut history = History::new();
        let txn = Transaction::new(
            vec![Edit::Insert {
                at: 0,
                text: "ab".to_owned(),
            }],
            vec![(0, 0), (5, 5)],
            vec![(2, 2), (7, 7)],
        );
        history.commit(txn);
        assert_eq!(
            history.undo(&mut buffer),
            Some(vec![(0, 0), (5, 5)]),
            "undo restaura TODOS los carets"
        );
        assert_eq!(
            history.redo(&mut buffer),
            Some(vec![(2, 2), (7, 7)]),
            "redo restaura TODOS los carets"
        );
    }
}
