//! Render y widgets de chrome de Jorgenio Text.
//!
//! - [`gpu`]: contexto wgpu (device, queue, surface) y presentación de frames.
//! - [`text`]: font system + buffers de cosmic-text vía glyphon.
//! - [`scene`]: lista de dibujo por frame (clear + spans de texto en px lógicos).
//! - (M4+) `render`: pipeline de quads; `widgets`: tabs, gutter, paleta, etc.
pub mod gpu;
pub mod scene;
pub mod text;
