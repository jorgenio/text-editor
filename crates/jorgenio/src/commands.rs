//! Comandos con nombre (español) y su keymap (atajos exactos por tecla).
//!
//! El shell (`main.rs`) resuelve la tecla → comando con [`command_for`] y lo
//! ejecuta con `run_command`; la paleta de comandos (Ctrl+Shift+P) lista
//! [`COMMANDS`] y muestra el atajo de cada uno con [`keys_for`].

use winit::keyboard::{Key, ModifiersState};

/// Una tecla del keymap: `key` en minúsculas (solo `Key::Character`).
pub struct Binding {
    pub key: &'static str,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub command: &'static str,
}

/// Atajo exacto: TODOS los modificadores deben coincidir (ni más ni menos).
pub const fn bind(
    key: &'static str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    command: &'static str,
) -> Binding {
    Binding {
        key,
        ctrl,
        shift,
        alt,
        command,
    }
}

pub static KEYMAP: &[Binding] = &[
    bind("p", true, true, false, "palette.show"),
    bind("n", true, false, false, "file.new"),
    bind("o", true, false, false, "file.open"),
    bind("s", true, false, false, "file.save"),
    bind("s", true, true, false, "file.save_as"),
    bind("w", true, false, false, "file.close"),
    bind("z", true, false, false, "edit.undo"),
    bind("z", true, true, false, "edit.redo"),
    bind("y", true, false, false, "edit.redo"),
    bind("c", true, false, false, "edit.copy"),
    bind("x", true, false, false, "edit.cut"),
    bind("v", true, false, false, "edit.paste"),
    bind("a", true, false, false, "edit.select_all"),
    bind("d", true, false, false, "edit.select_next_occurrence"),
    bind("k", true, true, false, "edit.delete_line"),
    bind("d", true, true, false, "edit.duplicate_line"),
    // Ctrl+/ con y sin Shift: en teclados ES "/" nace con Shift y en US sin él;
    // ambos eventos ('/' con ctrl) alternan comentarios de línea.
    bind("/", true, false, false, "edit.comment"),
    bind("/", true, true, false, "edit.comment"),
    // En US Ctrl+Shift+/ produce '?' (mayúsculas de '/'); ese es el bloque.
    bind("?", true, true, false, "edit.comment_block"),
    bind("f", true, false, false, "find.show"),
    bind("h", true, false, false, "replace.show"),
    bind("\\", true, false, false, "view.split"),
    bind("\\", true, true, false, "view.unsplit"),
    bind(",", true, false, false, "settings.open"),
];

/// Un comando ejecutable desde la paleta.
pub struct Command {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
}

pub static COMMANDS: &[Command] = &[
    Command {
        id: "palette.show",
        name: "Mostrar paleta de comandos",
        category: "Paleta",
    },
    Command {
        id: "file.save",
        name: "Guardar archivo",
        category: "Archivo",
    },
    Command {
        id: "file.new",
        name: "Archivo nuevo",
        category: "Archivo",
    },
    Command {
        id: "file.open",
        name: "Abrir archivo…",
        category: "Archivo",
    },
    Command {
        id: "file.save_as",
        name: "Guardar como…",
        category: "Archivo",
    },
    Command {
        id: "file.close",
        name: "Cerrar pestaña",
        category: "Archivo",
    },
    Command {
        id: "edit.undo",
        name: "Deshacer",
        category: "Edición",
    },
    Command {
        id: "edit.redo",
        name: "Rehacer",
        category: "Edición",
    },
    Command {
        id: "edit.copy",
        name: "Copiar",
        category: "Edición",
    },
    Command {
        id: "edit.cut",
        name: "Cortar",
        category: "Edición",
    },
    Command {
        id: "edit.paste",
        name: "Pegar",
        category: "Edición",
    },
    Command {
        id: "edit.select_all",
        name: "Seleccionar todo",
        category: "Selección",
    },
    Command {
        id: "edit.select_next_occurrence",
        name: "Seleccionar siguiente coincidencia",
        category: "Selección",
    },
    Command {
        id: "edit.delete_line",
        name: "Borrar línea",
        category: "Línea",
    },
    Command {
        id: "edit.duplicate_line",
        name: "Duplicar línea",
        category: "Línea",
    },
    Command {
        id: "edit.comment",
        name: "Comentar/destacar",
        category: "Edición",
    },
    Command {
        id: "edit.comment_block",
        name: "Comentar bloque",
        category: "Edición",
    },
    Command {
        id: "find.show",
        name: "Buscar",
        category: "Buscar",
    },
    Command {
        id: "replace.show",
        name: "Reemplazar",
        category: "Buscar",
    },
    Command {
        id: "view.split",
        name: "Dividir panel en dos columnas",
        category: "Ver",
    },
    Command {
        id: "view.unsplit",
        name: "Unir paneles",
        category: "Ver",
    },
    Command {
        id: "settings.open",
        name: "Abrir preferencias (settings.json)",
        category: "Preferencias",
    },
];

/// Normaliza la tecla lógica a la forma del keymap (minúsculas).
/// Solo `Key::Character`; los `Named` no tienen atajos.
fn normalize_key(key: &Key) -> Option<String> {
    match key {
        Key::Character(text) => Some(text.to_lowercase()),
        _ => None,
    }
}

/// Tecla + modificadores exactos → id de comando (o `None` si no está en el
/// keymap). Ctrl equivale a Super/Cmd; Alt debe coincidir tal cual (por eso
/// AltGr —Ctrl+Alt en Windows— nunca dispara comandos, solo inserta símbolos).
pub fn command_for(modifiers: ModifiersState, key: &Key) -> Option<&'static str> {
    let normalized = normalize_key(key)?;
    let ctrl = modifiers.control_key() || modifiers.super_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();
    KEYMAP
        .iter()
        .find(|binding| {
            binding.key == normalized
                && binding.ctrl == ctrl
                && binding.shift == shift
                && binding.alt == alt
        })
        .map(|binding| binding.command)
}

/// Atajo visible de un comando ("Ctrl+Shift+P"), si tiene binding.
pub fn keys_for(command: &str) -> Option<String> {
    let binding = KEYMAP.iter().find(|binding| binding.command == command)?;
    let mut out = String::new();
    if binding.ctrl {
        out.push_str("Ctrl+");
    }
    if binding.alt {
        out.push_str("Alt+");
    }
    if binding.shift {
        out.push_str("Shift+");
    }
    out.push_str(&binding.key.to_uppercase());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    fn mods(ctrl: bool, shift: bool, alt: bool) -> ModifiersState {
        let mut state = ModifiersState::empty();
        state.set(ModifiersState::CONTROL, ctrl);
        state.set(ModifiersState::SHIFT, shift);
        state.set(ModifiersState::ALT, alt);
        state
    }

    fn key(ch: char) -> Key {
        Key::Character(ch.to_string().into())
    }

    #[test]
    fn exact_modifiers_win() {
        assert_eq!(
            command_for(mods(true, false, false), &key('s')),
            Some("file.save")
        );
        assert_eq!(
            command_for(mods(true, true, false), &key('z')),
            Some("edit.redo")
        );
        assert_eq!(command_for(mods(false, false, false), &key('s')), None);
        assert_eq!(
            command_for(mods(true, true, false), &key('s')),
            Some("file.save_as")
        );
        // AltGr (Ctrl+Alt) no dispara nada: debe insertar el símbolo.
        assert_eq!(command_for(mods(true, false, true), &key('s')), None);
        assert_eq!(command_for(mods(true, true, true), &key('p')), None);
    }

    #[test]
    fn super_equals_ctrl() {
        let mut state = ModifiersState::empty();
        state.set(ModifiersState::SUPER, true);
        assert_eq!(command_for(state, &key('c')), Some("edit.copy"));
    }

    #[test]
    fn comment_bindings_cover_both_layouts() {
        // US: Ctrl+/ (sin Shift) → comentar.
        assert_eq!(
            command_for(mods(true, false, false), &key('/')),
            Some("edit.comment")
        );
        // ES: "/" nace con Shift (Ctrl+Shift+7) → también comentar.
        assert_eq!(
            command_for(mods(true, true, false), &key('/')),
            Some("edit.comment")
        );
        // US: Ctrl+Shift+/ produce '?' → comentario de bloque.
        assert_eq!(
            command_for(mods(true, true, false), &key('?')),
            Some("edit.comment_block")
        );
        assert_eq!(command_for(mods(true, false, false), &key('?')), None);
        assert_eq!(keys_for("edit.comment").as_deref(), Some("Ctrl+/"));
    }

    #[test]
    fn named_keys_have_no_command() {
        assert_eq!(
            command_for(mods(true, false, false), &Key::Named(NamedKey::ArrowLeft)),
            None
        );
    }

    #[test]
    fn keys_for_formats_first_binding() {
        assert_eq!(keys_for("file.save").as_deref(), Some("Ctrl+S"));
        assert_eq!(keys_for("palette.show").as_deref(), Some("Ctrl+Shift+P"));
        assert_eq!(keys_for("edit.redo").as_deref(), Some("Ctrl+Shift+Z"));
        assert_eq!(keys_for("view.split").as_deref(), Some("Ctrl+\\"));
        assert_eq!(keys_for("view.unsplit").as_deref(), Some("Ctrl+Shift+\\"));
        assert_eq!(keys_for("settings.open").as_deref(), Some("Ctrl+,"));
        assert_eq!(keys_for("missing.command"), None);
    }

    #[test]
    fn every_command_has_a_binding() {
        for command in COMMANDS {
            assert!(keys_for(command.id).is_some(), "sin atajo: {}", command.id);
        }
    }
}
