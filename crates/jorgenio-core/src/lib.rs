//! Núcleo de edición de Jorgenio Text.
//!
//! Modelo de edición puro (sin dependencias de UI): buffer sobre `ropey`,
//! movimiento de caret, mapeos de columna visual y undo/redo transaccional.
//! Todo testeable con `cargo test -p jorgenio-core`.
//!
//! Próximos hitos: selecciones multi-cursor (M5), sistema de comandos con
//! nombre y keymaps (M6).

pub mod buffer;
pub mod history;
pub mod movement;
