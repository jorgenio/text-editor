//! Núcleo de edición de Jorgenio Text.
//!
//! Modelo de edición puro, sin dependencias de UI: buffer (`ropey`),
//! selecciones multi-cursor, transacciones invertibles (undo/redo),
//! sistema de comandos con nombre y keymaps JSON.
//!
//! Este crate no depende de winit/wgpu: todo es testeable con `cargo test`.
