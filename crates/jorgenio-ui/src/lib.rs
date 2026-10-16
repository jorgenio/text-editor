//! Render y widgets de chrome de Jorgenio Text.
//!
//! - [`gpu`]: contexto wgpu (device, queue, surface) y frame a clear color.
//!   En M1 se le añade el text renderer de glyphon.
//! - (M4+) `render`: pipeline de quads + glyphon; `widgets`: tabs, gutter,
//!   paleta, find, minimap, splits.
pub mod gpu;
