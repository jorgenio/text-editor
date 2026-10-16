//! Historial undo/redo transaccional.
//!
//! Cada acción de edición genera una [`Transaction`] con sus edits inversos y
//! los carets antes/después. Las pulsaciones contiguas de tecleo o backspace
//! se fusionan (coalesce) para que Ctrl+Z deshaga bloques naturales en lugar
//! de un carácter a la vez.

use crate::buffer::{Buffer, Edit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub edits: Vec<Edit>,
    pub caret_before: usize,
    pub anchor_before: usize,
    pub caret_after: usize,
    pub anchor_after: usize,
}

impl Transaction {
    /// `before`/`after` = `(caret, anchor)` antes y después del cambio.
    pub fn new(edits: Vec<Edit>, before: (usize, usize), after: (usize, usize)) -> Self {
        Self {
            edits,
            caret_before: before.0,
            anchor_before: before.1,
            caret_after: after.0,
            anchor_after: after.1,
        }
    }

    /// Transacción inversa (aplicarla deshace el cambio).
    pub fn inverse(&self) -> Self {
        Self {
            edits: self.edits.iter().rev().map(Edit::inverse).collect(),
            caret_before: self.caret_after,
            anchor_before: self.anchor_after,
            caret_after: self.caret_before,
            anchor_after: self.anchor_before,
        }
    }

    pub fn undo_position(&self) -> (usize, usize) {
        (self.caret_before, self.anchor_before)
    }

    pub fn redo_position(&self) -> (usize, usize) {
        (self.caret_after, self.anchor_after)
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
            prev.caret_after = txn.caret_after;
            prev.anchor_after = txn.anchor_after;
            return;
        }
        self.undo_stack.push(txn);
    }

    /// Aplica el inverse del último txn; devuelve `(caret, anchor)` a restaurar.
    pub fn undo(&mut self, buffer: &mut Buffer) -> Option<(usize, usize)> {
        let txn = self.undo_stack.pop()?;
        for edit in txn.inverse().edits {
            edit.apply(buffer);
        }
        let position = txn.undo_position();
        self.redo_stack.push(txn);
        Some(position)
    }

    /// Reaplica el último txn deshecho; devuelve `(caret, anchor)` a restaurar.
    pub fn redo(&mut self, buffer: &mut Buffer) -> Option<(usize, usize)> {
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
            (at, at),
            (at + text.chars().count(), at + text.chars().count()),
        )
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut buffer = Buffer::new();
        let mut history = History::new();

        let txn = txn_insert(0, "hola");
        let edits = txn.edits.clone();
        buffer.insert(0, "hola");
        history.commit(txn);

        let position = history.undo(&mut buffer).expect("debe poder deshacer");
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "");
        assert_eq!(position, (0, 0));

        let position = history.redo(&mut buffer).expect("debe poder rehacer");
        assert_eq!(buffer.text_range(0..buffer.len_chars()), "hola");
        assert_eq!(position, (4, 4));

        // Las aplicaciones manuales en el test no deben enmascarar el inverse:
        assert_eq!(edits.len(), 1);
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
            (4, 4),
            (2, 2),
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
        // "hola": backspace en 4 → Delete{3,"a"}; en 3 → Delete{2,"l"}.
        history.commit(Transaction::new(
            vec![Edit::Delete {
                at: 3,
                text: "a".to_owned(),
            }],
            (4, 4),
            (3, 3),
        ));
        history.commit(Transaction::new(
            vec![Edit::Delete {
                at: 2,
                text: "l".to_owned(),
            }],
            (3, 3),
            (2, 2),
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
}
