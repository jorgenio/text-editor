//! Panel de salida del build (M28): lanza la tarea elegida de `builds` en
//! un shell, recoge su stdout/stderr en un hilo (la app no se congela) y
//! pinta las líneas al pie sobre la barra de estado, con `ruta:línea`
//! clicables para saltar al error (como el panel de buscar en archivos).
//!
//! El hilo principal solo DRENA el canal en [`BuildPanel::poll`] (lo llama
//! `main.rs` en `about_to_wait`, con un sondeo de 50 ms mientras corre) y
//! Esc cierra el panel. Las rutas relativas se resuelven contra el cwd de
//! la tarea.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use jorgenio_ui::scene::{Quad, Scene, TextSpan};

use crate::editor::FONT_SIZE;
use crate::settings::BuildTask;
use crate::theme::Theme;

/// Altura de la cabecera con el nombre de la tarea y su estado.
const HEAD_H: f32 = 24.0;
/// Altura de cada línea de salida.
const ROW_H: f32 = 20.0;
/// Líneas visibles como máximo.
const MAX_ROWS: usize = 14;
/// Líneas retenidas en memoria (el exceso se descarta por el principio).
const MAX_LINES: usize = 2_000;
/// Margen horizontal dentro del panel.
const PAD: f32 = 8.0;
/// Franja de borde superior del panel.
const EDGE: f32 = 1.0;
/// Sondeo del canal mientras corre el build (ms): el mpsc no despierta
/// solo al event loop, así que `about_to_wait` pide un wakeup periódico.
pub const POLL_MS: u64 = 50;

/// Rectángulo para el hit-test del ratón.
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Qué toca el ratón dentro del panel ([`BuildPanel::click`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    /// Fuera: el click sigue (el editor u otra capa) y el panel se queda.
    Miss,
    /// Dentro pero sin fila utilizable: se traga.
    Panel,
    /// Una fila con `ruta:línea`: abrirla.
    Row(PathBuf, usize),
}

/// Una línea de salida (con su origen, para colorearla).
struct Line {
    text: String,
    stderr: bool,
}

/// Estado del proceso (se muestra en la cabecera).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Running,
    /// Terminó con este código (`None` = terminado por señal).
    Exited(Option<i32>),
    /// No se pudo lanzar el proceso (o el hilo murió sin código).
    Failed,
}

/// Lo que el hijo manda al hilo principal.
enum Event {
    /// Línea de stdout (`stderr: false`) o de stderr (`true`).
    Line { text: String, stderr: bool },
    /// El proceso terminó con este código.
    Finished { code: Option<i32> },
}

/// Layout del último render (para click y rueda).
#[derive(Clone, Copy)]
struct Layout {
    panel: Rect,
    /// Y donde empiezan las filas de salida.
    rows_y: f32,
    /// Filas visibles dibujadas.
    rows: usize,
}

#[derive(Default)]
pub struct BuildPanel {
    open: bool,
    /// Nombre de la tarea en curso (o la última lanzada).
    name: String,
    status: Option<Status>,
    lines: VecDeque<Line>,
    scroll: usize,
    /// cwd de la tarea: contra él se resuelven las rutas relativas.
    cwd: PathBuf,
    /// Canal con el hijo; `None` = nada en vuelo.
    rx: Option<Receiver<Event>>,
    last: Option<Layout>,
    /// Avance del último render, para truncar líneas largas.
    advance: f32,
}

impl BuildPanel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Some(Status::Running))
    }

    pub fn close(&mut self) {
        self.open = false;
        self.last = None;
    }

    /// Lanza `task.cmd` en un shell con cwd `cwd`: limpia la salida
    /// anterior, abre el panel y deja el canal de eventos listo. Un fallo
    /// de spawn se refleja en el panel (línea de error) Y se devuelve.
    pub fn start(&mut self, task: &BuildTask, cwd: PathBuf) -> std::io::Result<()> {
        self.open = true;
        self.name = task.name.clone();
        self.lines.clear();
        self.scroll = 0;
        self.cwd = cwd.clone();
        self.last = None;
        let mut child = match spawn_shell(&task.cmd, &cwd) {
            Ok(child) => child,
            Err(err) => {
                self.status = Some(Status::Failed);
                self.rx = None;
                self.push_line(Line {
                    text: format!("No se pudo lanzar «{}»: {err}", task.cmd),
                    stderr: true,
                });
                return Err(err);
            }
        };
        let (tx, rx) = mpsc::channel();
        if let Some(pipe) = child.stdout.take() {
            reader_thread(pipe, tx.clone(), false);
        }
        if let Some(pipe) = child.stderr.take() {
            reader_thread(pipe, tx.clone(), true);
        }
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|status| status.code());
            let _ = tx.send(Event::Finished { code });
        });
        self.status = Some(Status::Running);
        self.rx = Some(rx);
        Ok(())
    }

    /// Drena el canal sin bloquear; `true` = hubo novedades (repintar).
    /// Al terminar suelta el canal; si el hilo muere sin `Finished`, el
    /// panel queda en `Failed` en vez de sondear para siempre.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.take() else {
            return false;
        };
        let mut changed = false;
        let mut done = false;
        loop {
            match rx.try_recv() {
                Ok(Event::Line { text, stderr }) => {
                    let text = strip_ansi(&text);
                    self.push_line(Line { text, stderr });
                    changed = true;
                }
                Ok(Event::Finished { code }) => {
                    self.status = Some(Status::Exited(code));
                    done = true;
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if matches!(self.status, Some(Status::Running)) {
                        self.status = Some(Status::Failed);
                    }
                    done = true;
                    changed = true;
                    break;
                }
            }
        }
        if done {
            self.rx = None;
        } else {
            self.rx = Some(rx);
        }
        changed
    }

    /// Salida acumulada, una línea por fila (solo tests: en la app el texto
    /// ya está en `lines`, que es lo que pinta `render`).
    #[cfg(test)]
    pub fn output(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Añade una línea al final y recorta el exceso por el principio.
    fn push_line(&mut self, line: Line) {
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
            self.scroll = self.scroll.saturating_sub(1);
        }
    }

    /// Dibuja el panel sobre la franja inferior (`bottom` = alto de la
    /// barra de estado) y guarda el layout para click y rueda.
    pub fn render(
        &mut self,
        scene: &mut Scene,
        viewport: (f32, f32),
        advance: f32,
        bottom: f32,
        theme: &Theme,
    ) {
        if !self.open {
            self.last = None;
            return;
        }
        self.advance = advance;
        let visible = self.lines.len().saturating_sub(self.scroll).min(MAX_ROWS);
        let panel_h = EDGE + HEAD_H + visible as f32 * ROW_H;
        let panel_y = (viewport.1 - bottom - panel_h).max(0.0);
        let panel = Rect::new(0.0, panel_y, viewport.0, panel_h);
        scene.top_quads.push(Quad::new(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            theme.panel_bg,
        ));
        scene
            .top_quads
            .push(Quad::new(0.0, panel_y, viewport.0, EDGE, theme.panel_edge));

        // Cabecera: nombre de la tarea a la izquierda, estado a la derecha.
        let head_y = panel_y + EDGE;
        scene.top_texts.push(TextSpan::new(
            format!("Build: {}", self.name),
            PAD,
            head_y + (HEAD_H - FONT_SIZE) / 2.0,
            FONT_SIZE,
            theme.fg,
        ));
        let (status_text, status_color) = match self.status {
            Some(Status::Running) => ("en curso…".to_owned(), theme.accent),
            Some(Status::Exited(Some(0))) => ("exit 0".to_owned(), theme.dim),
            Some(Status::Exited(Some(code))) => (format!("exit {code}"), theme.error),
            Some(Status::Exited(None)) => ("terminado sin código".to_owned(), theme.error),
            Some(Status::Failed) => ("no se pudo lanzar".to_owned(), theme.error),
            None => (String::new(), theme.dim),
        };
        let status_w = status_text.chars().count() as f32 * advance;
        scene.top_texts.push(TextSpan::new(
            status_text,
            viewport.0 - PAD - status_w,
            head_y + (HEAD_H - FONT_SIZE) / 2.0,
            FONT_SIZE,
            status_color,
        ));

        // Filas de salida visibles; stderr en rojo.
        let rows_y = head_y + HEAD_H;
        let capacity = ((viewport.0 - 2.0 * PAD) / advance).floor().max(4.0) as usize;
        for (row, line) in self
            .lines
            .iter()
            .skip(self.scroll)
            .take(MAX_ROWS)
            .enumerate()
        {
            let y = rows_y + row as f32 * ROW_H;
            let mut text = line.text.clone();
            if text.chars().count() > capacity {
                let head: String = text.chars().take(capacity.saturating_sub(1)).collect();
                text = format!("{head}\u{2026}");
            }
            scene.top_texts.push(TextSpan::new(
                text,
                PAD,
                y + (ROW_H - FONT_SIZE) / 2.0,
                FONT_SIZE,
                if line.stderr { theme.error } else { theme.fg },
            ));
        }

        self.last = Some(Layout {
            panel,
            rows_y,
            rows: visible,
        });
    }

    /// Click dentro del panel abierto (usa el layout del último render).
    pub fn click(&self, x: f32, y: f32) -> Click {
        let Some(layout) = self.last else {
            return Click::Miss; // aún sin render: se traga el primer click
        };
        if !layout.panel.contains(x, y) {
            return Click::Miss;
        }
        let row = ((y - layout.rows_y) / ROW_H).floor() as isize;
        if row >= 0
            && (row as usize) < layout.rows
            && let Some(line) = self.lines.get(self.scroll + row as usize)
            && let Some((path, line_no)) = parse_location(&line.text, &self.cwd)
        {
            return Click::Row(path, line_no);
        }
        Click::Panel
    }

    /// Rueda del ratón sobre el panel (`notches` = ranuras de rueda, con
    /// el signo del editor: + arriba). `true` = consumido.
    pub fn on_wheel(&mut self, x: f32, y: f32, notches: f32) -> bool {
        if !self.open {
            return false;
        }
        let Some(layout) = self.last else {
            return false;
        };
        if !layout.panel.contains(x, y) {
            return false;
        }
        // + arriba (como el editor): redondea a ranuras enteras y, con
        // deltas pequeños (touchpad), avanza al menos una línea.
        let mut rows = notches.round();
        if rows == 0.0 && notches != 0.0 {
            rows = notches.signum();
        }
        let delta = -(rows as isize);
        if delta != 0 {
            let max = self.lines.len().saturating_sub(MAX_ROWS);
            self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        }
        true
    }
}

/// `src/main.rs:12:5: error…`, ` --> C:\x\main.rs:12:5` o `ruta:12: aviso`
/// → `(ruta, línea)`. Las rutas relativas se resuelven contra `cwd`; la
/// primera unidad de Windows (`C:`) no se confunde con el separador porque
/// solo acepta un `:` tras el que siguen dígitos.
fn parse_location(line: &str, cwd: &Path) -> Option<(PathBuf, usize)> {
    // rustc escribe " --> ruta:línea:col"; el resto empieza por la ruta.
    let candidate = line.split_once("-->").map_or(line, |(_, rest)| rest);
    // Se avanza por índices (no recortando la cadena) para que la unidad de
    // Windows (`C:`) no desaparezca de la cabeza al descartar el primer `:`.
    let mut from = 0;
    while let Some(offset) = candidate[from..].find(':') {
        let colon = from + offset;
        let after = candidate[colon + 1..].trim_start();
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let head = candidate[..colon].trim();
            if head.contains('.') || head.contains('/') || head.contains('\\') {
                let number: usize = digits.parse().ok()?;
                if number == 0 {
                    return None;
                }
                let path = PathBuf::from(head);
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                return Some((path, number));
            }
        }
        from = colon + 1;
    }
    None
}

/// Quita las secuencias ANSI (`ESC[...m` y familia) de una línea de salida.
fn strip_ansi(text: &str) -> String {
    if !text.contains('\x1b') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // CSI: consume el cuerpo hasta la letra final (o fin de línea).
        if chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// Lanza `cmd` en el shell del sistema con cwd `cwd`, con las tres stdio
/// direccionadas a tuberías (y sin ventana de consola en Windows).
fn spawn_shell(cmd: &str, cwd: &Path) -> std::io::Result<std::process::Child> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(cmd);
        command
    } else {
        let mut command = Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // La GUI no tiene consola: sin este flag Windows abriría una ventana
    // parpadeando cada vez que se lanza el build.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn()
}

/// Hilo lector de una tubería: cada línea viaja al hilo principal.
fn reader_thread(pipe: impl std::io::Read + Send + 'static, tx: mpsc::Sender<Event>, stderr: bool) {
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            let _ = tx.send(Event::Line { text: line, stderr });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn scene() -> Scene {
        Scene::new(wgpu::Color::BLACK)
    }

    fn task(cmd: &str) -> BuildTask {
        BuildTask {
            name: "prueba".to_owned(),
            cmd: cmd.to_owned(),
            ..BuildTask::default()
        }
    }

    #[test]
    fn parse_location_reads_paths_lines_and_rustc_arrows() {
        let cwd = Path::new("/proy");
        assert_eq!(
            parse_location("src/main.rs:12:5: error: mal", cwd),
            Some((cwd.join("src/main.rs"), 12))
        );
        assert_eq!(
            parse_location(" --> src/lib.rs:3:1", cwd),
            Some((cwd.join("src/lib.rs"), 3)),
            "flecha de rustc"
        );
        if cfg!(windows) {
            assert_eq!(
                parse_location(r"C:\proy\a.rs:7: aviso", cwd),
                Some((PathBuf::from(r"C:\proy\a.rs"), 7)),
                "la unidad Windows no se traga"
            );
        }
        // En Windows "/abs/b.rs" no tiene unidad → no es absoluta y se
        // resuelve contra cwd; el resultado esperado replica ese mismo join.
        let expected_abs = cwd.join(Path::new("/abs").join("b.rs"));
        assert_eq!(
            parse_location("/abs/b.rs:1: x", cwd),
            Some((expected_abs, 1)),
            "absoluta no confunde con relativa"
        );
        assert_eq!(
            parse_location("error: algo falló: 5", cwd),
            None,
            "sin ruta"
        );
        assert_eq!(
            parse_location("src/a.rs:0: x", cwd),
            None,
            "línea 0 inválida"
        );
        assert_eq!(parse_location("hola: mundo", cwd), None, "sin dígitos");
    }

    #[test]
    fn strip_ansi_keeps_plain_text_and_drops_escapes() {
        assert_eq!(strip_ansi("normal"), "normal");
        assert_eq!(
            strip_ansi("\x1b[31merror\x1b[0m: rojo"),
            "error: rojo",
            "colores SGR fuera"
        );
        assert_eq!(strip_ansi("\x1b[1;32mok\x1b[m"), "ok", "secuencias con ';'");
    }

    #[test]
    fn render_stores_layout_and_click_opens_the_location() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut panel = BuildPanel {
            open: true,
            name: "prueba".to_owned(),
            cwd: dir.path().to_path_buf(),
            status: Some(Status::Exited(Some(1))),
            ..BuildPanel::default()
        };
        panel.lines.push_back(Line {
            text: "src/main.rs:12:5: error: mal".to_owned(),
            stderr: true,
        });
        panel.lines.push_back(Line {
            text: "compilación fallida".to_owned(),
            stderr: false,
        });

        let mut s = scene();
        panel.render(&mut s, (800.0, 600.0), 8.0, 24.0, &Theme::dark());
        let layout = panel.last.expect("render guarda el layout");
        assert!(
            (layout.panel.y + layout.panel.h - (600.0 - 24.0)).abs() < 0.01,
            "justo encima de la barra de estado"
        );
        assert_eq!(layout.rows, 2, "dos líneas");
        assert!(
            s.top_texts.iter().any(|t| t.text == "Build: prueba"),
            "cabecera con el nombre"
        );
        assert!(
            s.top_texts.iter().any(|t| t.text == "exit 1"),
            "estado en la cabecera"
        );

        // Fila con ubicación → abrir; fila sin ubicación y cabecera → traga.
        let y = layout.rows_y + ROW_H / 2.0;
        assert_eq!(
            panel.click(100.0, y),
            Click::Row(dir.path().join("src/main.rs"), 12)
        );
        let y2 = layout.rows_y + ROW_H + ROW_H / 2.0;
        assert_eq!(panel.click(100.0, y2), Click::Panel, "sin ruta: se traga");
        assert_eq!(panel.click(100.0, layout.panel.y + 2.0), Click::Panel);
        // Fuera del panel → Miss (el click sigue, el panel se queda).
        assert_eq!(panel.click(100.0, 590.0), Click::Miss);
        // Cerrado: sin render no hay dónde picar.
        panel.close();
        assert_eq!(panel.click(100.0, y), Click::Miss);
    }

    #[test]
    fn wheel_scrolls_only_over_the_panel() {
        let mut panel = BuildPanel {
            open: true,
            ..BuildPanel::default()
        };
        for i in 0..40 {
            panel.lines.push_back(Line {
                text: format!("linea {i}"),
                stderr: false,
            });
        }
        let mut s = scene();
        panel.render(&mut s, (800.0, 600.0), 8.0, 24.0, &Theme::dark());
        let layout = panel.last.expect("layout");

        assert!(
            panel.on_wheel(100.0, layout.rows_y + 4.0, 1.0),
            "rueda arriba"
        );
        assert_eq!(panel.scroll, 0, "arriba del todo no sube más");
        assert!(
            panel.on_wheel(100.0, layout.rows_y + 4.0, -1.0),
            "rueda abajo"
        );
        assert_eq!(panel.scroll, 1, "una ranura = una línea");
        // Fondo: 40 líneas con ventana de MAX_ROWS.
        panel.on_wheel(100.0, layout.rows_y + 4.0, -999.0);
        assert_eq!(panel.scroll, 40 - MAX_ROWS, "tope al final");
        // Fuera del panel: no consume la rueda (la recoge el editor).
        assert!(!panel.on_wheel(100.0, 100.0, -1.0));
        panel.close();
        assert!(!panel.on_wheel(100.0, layout.rows_y + 4.0, -1.0));
    }

    #[test]
    fn start_runs_a_shell_and_poll_collects_its_output() {
        let mut panel = BuildPanel::default();
        panel
            .start(&task("echo hola build"), std::env::current_dir().unwrap())
            .expect("spawn");
        assert!(panel.is_open(), "el panel se abre solo");
        assert!(panel.is_running(), "arranca en su hilo");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut changed = false;
        while panel.is_running() && Instant::now() < deadline {
            changed |= panel.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!panel.is_running(), "termina solo");
        // El waiter puede mandar Finished antes de que el lector suelte la
        // última línea: se drena un poco más antes de mirar.
        for _ in 0..25 {
            if !panel.poll() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(changed, "hubo novedades que drenar");
        assert!(panel.output().contains("hola build"), "{}", panel.output());
        assert_eq!(panel.status, Some(Status::Exited(Some(0))), "echo: exit 0");
        assert!(panel.rx.is_none(), "el canal se suelta al terminar");
    }

    #[test]
    fn spawn_failure_marks_failed_and_stays_visible() {
        // cwd que no existe: el spawn falla y el panel lo cuenta.
        let bad = std::env::temp_dir().join("jorgenio_cwd_que_no_existe_asi");
        let mut panel = BuildPanel::default();
        let err = panel
            .start(&task("echo imposible"), bad)
            .expect_err("cwd inexistente");
        assert!(panel.is_open(), "el error se ve en el panel");
        assert_eq!(panel.status, Some(Status::Failed));
        assert!(
            panel.output().contains("No se pudo lanzar"),
            "{}",
            panel.output()
        );
        assert!(!panel.is_running(), "no queda sondeando");
        let _ = err;
    }

    #[test]
    fn poll_caps_stored_lines_at_the_maximum() {
        let mut panel = BuildPanel::default();
        for i in 0..(MAX_LINES + 10) {
            panel.push_line(Line {
                text: format!("l{i}"),
                stderr: false,
            });
        }
        assert_eq!(panel.lines.len(), MAX_LINES, "tope de memoria");
        assert_eq!(
            panel.lines.front().map(|l| l.text.as_str()),
            Some("l10"),
            "se descarta lo viejo"
        );
    }
}
