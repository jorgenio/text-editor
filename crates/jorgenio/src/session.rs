//! Estado de sesión entre arranques (NO es preferencia editable): el
//! tamaño de la ventana y el hot exit (M23), los buffers sin guardar.
//! Se guarda al cerrar y se lee al arrancar, junto a `settings.json`
//! (respeta `JORGENIO_CONFIG_DIR`). Un archivo ausente o ilegible usa los
//! defaults: nunca pánic.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Tope de pestañas sucias que el hot exit guarda en la sesión.
pub const HOT_MAX_BUFFERS: usize = 20;
/// Tope de bytes de texto por buffer en la sesión (más grande se omite:
/// un archivo enorme no debe reventar el arranque ni el disco).
pub const HOT_MAX_TEXT: usize = 1024 * 1024;

/// Un buffer del hot exit: su ruta (None = sin título) y el texto tal
/// como quedó al cerrar (puede no coincidir con el disco).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotBuffer {
    pub path: Option<String>,
    pub text: String,
}

/// Tamaño lógico de la ventana recordado entre sesiones (+ hot exit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub window_width: u32,
    pub window_height: u32,
    /// Pestañas sin guardar (hot exit); vacío = nada que restaurar.
    pub buffers: Vec<HotBuffer>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            window_width: 1200,
            window_height: 800,
            buffers: Vec::new(),
        }
    }
}

impl Session {
    /// Tamaño recortado a un rango usable (640x400 .. 7680x4320).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            window_width: width.clamp(640, 7680),
            window_height: height.clamp(400, 4320),
            buffers: Vec::new(),
        }
    }

    /// Fija los buffers del hot exit aplicando los límites: se descartan
    /// los vacíos (nada que restaurar) y los de más de [`HOT_MAX_TEXT`]
    /// bytes (no truncar: partir un char UTF-8 o un buffer a medias sería
    /// peor que no restaurarlo), y se quedan los primeros
    /// [`HOT_MAX_BUFFERS`]. También lo usa la carga: un session.json a mano
    /// con un buffer gigante no revienta el arranque.
    pub fn set_buffers(&mut self, buffers: Vec<HotBuffer>) {
        self.buffers = buffers
            .into_iter()
            .filter(|buffer| !buffer.text.is_empty())
            .filter(|buffer| {
                let too_big = buffer.text.len() > HOT_MAX_TEXT;
                if too_big {
                    log::warn!(
                        "hot exit: buffer de {} bytes supera el tope; no se guarda",
                        buffer.text.len()
                    );
                }
                !too_big
            })
            .take(HOT_MAX_BUFFERS)
            .collect();
    }
}

/// Ruta de `session.json` (junto a `settings.json`).
pub fn path() -> PathBuf {
    crate::settings::path().with_file_name("session.json")
}

fn read_session(target: &Path) -> Session {
    match std::fs::read_to_string(target) {
        Ok(text) => match serde_json::from_str::<Session>(&text) {
            Ok(raw) => {
                let mut session = Session::new(raw.window_width, raw.window_height);
                session.set_buffers(raw.buffers);
                session
            }
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
                window_height: 800,
                buffers: Vec::new()
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
                window_height: 900,
                buffers: Vec::new()
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
                window_height: 900,
                buffers: Vec::new()
            },
            "guarda y relee"
        );

        std::fs::write(&target, "{ mal").expect("escribe");
        assert_eq!(read_session(&target), Session::default(), "JSON malo");
    }

    #[test]
    fn session_without_buffers_field_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("session.json");
        // session.json de antes del hot exit (M22): solo el tamaño.
        std::fs::write(&target, r#"{"window_width": 900, "window_height": 700}"#).expect("escribe");

        let session = read_session(&target);
        assert_eq!(session.window_width, 900);
        assert_eq!(session.window_height, 700);
        assert!(session.buffers.is_empty(), "campo ausente → default");
    }

    #[test]
    fn hot_exit_roundtrips_dirty_buffers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("session.json");

        let mut session = Session::new(1200, 800);
        session.set_buffers(vec![
            HotBuffer {
                path: Some("notas.md".to_owned()),
                text: "borrador sin guardar".to_owned(),
            },
            HotBuffer {
                path: None,
                text: "sin título también".to_owned(),
            },
        ]);
        write_session(&target, &session);

        let reloaded = read_session(&target);
        assert_eq!(reloaded.buffers, session.buffers, "buffers idénticos");
        assert_eq!(reloaded.window_width, 1200);
    }

    #[test]
    fn set_buffers_applies_limits() {
        let mut session = Session::default();
        let big = "x".repeat(HOT_MAX_TEXT + 1);
        session.set_buffers(vec![
            HotBuffer {
                path: None,
                text: String::new(),
            },
            HotBuffer {
                path: None,
                text: big,
            },
            HotBuffer {
                path: Some("a.rs".to_owned()),
                text: "uno".to_owned(),
            },
        ]);
        assert_eq!(session.buffers.len(), 1, "vacío y gigante descartados");
        assert_eq!(session.buffers[0].text, "uno");

        let many: Vec<HotBuffer> = (0..HOT_MAX_BUFFERS + 5)
            .map(|i| HotBuffer {
                path: None,
                text: format!("buf{i}"),
            })
            .collect();
        session.set_buffers(many);
        assert_eq!(session.buffers.len(), HOT_MAX_BUFFERS, "tope de pestañas");
        assert_eq!(session.buffers[0].text, "buf0", "se quedan los primeros");
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
