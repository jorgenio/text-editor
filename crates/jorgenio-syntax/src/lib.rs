//! Syntax highlighting de Jorgenio Text.
//!
//! Envuelve `syntect` (que habla los `.sublime-syntax` y `.tmTheme`
//! originales de Sublime Text) con una cache de estados por línea para
//! resaltar solo las líneas visibles.
