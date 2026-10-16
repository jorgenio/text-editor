//! Render y widgets de chrome de Jorgenio Text.
//!
//! - [`gpu`]: contexto wgpu (device, queue, surface) y presentación de frames.
//! - [`text`]: font system + buffers de cosmic-text vía glyphon.
//! - [`scene`]: lista de dibujo por frame (clear + quads + spans en px lógicos).
//! - [`quads`]: pipeline instanciado de rectángulos (selecciones, carets).
//! - (M4+) `widgets`: tabs, gutter chrome, paleta, etc.
pub mod gpu;
pub mod quads;
pub mod scene;
pub mod text;
