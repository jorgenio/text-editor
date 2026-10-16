//! Núcleo de edición de Jorgenio Text.
//!
//! Modelo de edición puro (sin dependencias de UI): buffer sobre `ropey`,
//! movimiento de caret y mapeos de columna visual. Todo testeable con
//! `cargo test -p jorgenio-core`.
//!
//! Próximos hitos: selecciones multi-cursor y transacciones undo/redo (M3),
//! sistema de comandos con nombre y keymaps (M6).

pub mod buffer;
pub mod movement;
