//! Estado de sesión entre arranques (NO es preferencia editable): el
//! tamaño de la ventana. Se guarda al cerrar y se lee al arrancar, junto
//! a `settings.json` (respeta `JORGENIO_CONFIG_DIR`). Un archivo ausente
//! o ilegible usa los defaults: nunca pánic.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Tamaño lógico de la ventana recordado entre sesiones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub window_width: u32,
    pub window_height: u32,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            window_width: 1200,
            window_height: 800,
        }
    }
}

impl Session {
    /// Tamaño recortado a un rango usable (640x400 .. 7680x4320).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            window_width: width.clamp(640, 7680),
            window_height: height.clamp(400, 4320),
        }
    }
}

/// Ruta de `session.json` (junto a `settings.json`).
pub fn path() -> PathBuf {
    crate::settings::path().with_file_name("session.json")
}

fn read_session(target: &Path) -> Session {
    match std::fs::read_to_string(target) {
        Ok(text) => match serde_json::from_str::<Session>(&text) {
            Ok(raw) => Session::new(raw.window_width, raw.window_height),
            Err(err) => {
                log::warn!("{} ilegible ({err}); usando defaults", target.display());
                Session::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Session::default(),
        Err(err) => {
            log::warn!("no se pudo leer {}: {err}", target.display());
            Session::default()
        }
    }
}

/// Carga la sesión (si falta o falla → defaults, nunca pánic).
pub fn load() -> Session {
    read_session(&path())
}

fn write_session(target: &Path, session: &Session) {
    let Ok(json) = serde_json::to_string_pretty(session) else {
        return;
    };
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(err) = std::fs::write(target, json) {
        log::warn!("no se pudo guardar {}: {err}", target.display());
    }
}

/// Guarda la sesión (al cerrar la ventana; los errores solo se loguean).
pub fn save(session: &Session) {
    write_session(&path(), session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_initial_window() {
        assert_eq!(
            Session::default(),
            Session {
                window_width: 1200,
                window_height: 800
            }
        );
    }

    #[test]
    fn new_clamps_to_a_usable_range() {
        assert_eq!(Session::new(640, 400), Session::new(1, 1), "suelo");
        assert_eq!(Session::new(640, 400).window_width, 640);
        assert_eq!(Session::new(640, 400).window_height, 400);
        let huge = Session::new(99_999, 99_999);
        assert_eq!(huge.window_width, 7680, "techo");
        assert_eq!(huge.window_height, 4320);
        assert_eq!(
            Session::new(1440, 900),
            Session {
                window_width: 1440,
                window_height: 900
            },
            "tamaños normales pasan tal cual"
        );
    }

    #[test]
    fn roundtrip_and_fallbacks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("session.json");

        assert_eq!(read_session(&target), Session::default(), "sin archivo");

        write_session(&target, &Session::new(1600, 900));
        assert_eq!(
            read_session(&target),
            Session {
                window_width: 1600,
                window_height: 900
            },
            "guarda y relee"
        );

        std::fs::write(&target, "{ mal").expect("escribe");
        assert_eq!(read_session(&target), Session::default(), "JSON malo");
    }

    #[test]
    fn path_sits_next_to_settings() {
        assert_eq!(
            path().file_name(),
            Some(std::ffi::OsStr::new("session.json"))
        );
        assert_eq!(path().parent(), crate::settings::path().parent());
    }
}
