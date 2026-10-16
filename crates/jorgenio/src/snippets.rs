//! Snippets básicos (M12): Tab tras una palabra-clave expande una plantilla.
//! Solo `$0` marca dónde queda el caret; `${n:texto}` deja su texto tal cual
//! (sin navegación de tabstops en v1).

pub struct Snippet {
    pub trigger: &'static str,
    pub body: &'static str,
}

const RS: &[Snippet] = &[
    Snippet {
        trigger: "fn",
        body: "fn ${1:nombre}() {\n    $0\n}",
    },
    Snippet {
        trigger: "if",
        body: "if ${1:cond} {\n    $0\n}",
    },
    Snippet {
        trigger: "for",
        body: "for ${1:item} in ${2:iterable} {\n    $0\n}",
    },
    Snippet {
        trigger: "while",
        body: "while ${1:cond} {\n    $0\n}",
    },
    Snippet {
        trigger: "loop",
        body: "loop {\n    $0\n}",
    },
    Snippet {
        trigger: "test",
        body: "#[test]\nfn ${1:nombre}() {\n    $0\n}",
    },
    Snippet {
        trigger: "impl",
        body: "impl ${1:Tipo} {\n    $0\n}",
    },
    Snippet {
        trigger: "main",
        body: "fn main() {\n    $0\n}",
    },
];

const JS: &[Snippet] = &[
    Snippet {
        trigger: "fn",
        body: "function ${1:nombre}() {\n    $0\n}",
    },
    Snippet {
        trigger: "if",
        body: "if (${1:cond}) {\n    $0\n}",
    },
    Snippet {
        trigger: "for",
        body: "for (const ${1:item} of ${2:lista}) {\n    $0\n}",
    },
    Snippet {
        trigger: "while",
        body: "while (${1:cond}) {\n    $0\n}",
    },
    Snippet {
        trigger: "log",
        body: "console.log($0);",
    },
];

const PY: &[Snippet] = &[
    Snippet {
        trigger: "def",
        body: "def ${1:nombre}(${2}):\n    $0",
    },
    Snippet {
        trigger: "if",
        body: "if ${1:cond}:\n    $0",
    },
    Snippet {
        trigger: "for",
        body: "for ${1:item} in ${2:iterable}:\n    $0",
    },
    Snippet {
        trigger: "while",
        body: "while ${1:cond}:\n    $0",
    },
];

/// Cuerpo del snippet `trigger` para la extensión `lang` (sin snippets → `None`).
pub fn body_for(lang: &str, trigger: &str) -> Option<&'static str> {
    list_for(lang)
        .iter()
        .find(|snippet| snippet.trigger == trigger)
        .map(|snippet| snippet.body)
}

/// Expande el cuerpo: quita `$0`/`${n:…}` y devuelve `(texto, caret en chars)`.
pub fn render(body: &str) -> (String, usize) {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut caret: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('0') => {
                if caret.is_none() {
                    caret = Some(out.chars().count());
                }
                i += 2;
            }
            Some('{') => {
                let close = chars[i + 2..].iter().position(|&c| c == '}');
                match close {
                    Some(rel) => {
                        let inner: String = chars[i + 2..i + 2 + rel].iter().collect();
                        if let Some((_var, text)) = inner.split_once(':') {
                            out.push_str(text);
                        }
                        i += 2 + rel + 1;
                    }
                    None => {
                        out.push('$'); // sin cerrar: carácter literal
                        i += 1;
                    }
                }
            }
            Some(&digit) if digit.is_ascii_digit() => {
                // `${n}` sin texto: lo omite entero.
                i += 2; // '$' y el primer dígito
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    let end = out.chars().count();
    (out, caret.unwrap_or(end))
}

fn list_for(lang: &str) -> &'static [Snippet] {
    match lang {
        "rs" => RS,
        "js" | "ts" | "jsx" | "tsx" => JS,
        "py" => PY,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_places_caret_and_strips_placeholders() {
        let (text, caret) = render("fn ${1:nombre}() {\n    $0\n}");
        assert_eq!(text, "fn nombre() {\n    \n}");
        assert_eq!(caret, 18, "caret justo tras 'fn nombre() {{\\n    '");
    }

    #[test]
    fn render_keeps_placeholder_text_without_caret_marker() {
        let (text, caret) = render("for ${1:item} in ${2:iterable} {}");
        assert_eq!(text, "for item in iterable {}");
        assert_eq!(caret, text.chars().count(), "sin $0 → al final");
    }

    #[test]
    fn render_tolerates_unknown_placeholders() {
        assert_eq!(render("$x").0, "$x");
        assert_eq!(render("${unclosed").0, "${unclosed");
        assert_eq!(render("$1$22").0, "");
    }

    #[test]
    fn body_for_gates_by_language() {
        assert_eq!(body_for("rs", "fn"), Some("fn ${1:nombre}() {\n    $0\n}"));
        assert_eq!(body_for("js", "log"), Some("console.log($0);"));
        assert_eq!(
            body_for("py", "def"),
            Some("def ${1:nombre}(${2}):\n    $0")
        );
        assert_eq!(
            body_for("py", "fn"),
            None,
            "sin snippets para esa extensión"
        );
        assert_eq!(body_for("", "fn"), None);
    }
}
