//! Terminal-based UI replacing the original Qt `MainWindow`.
//!
//! Layout:
//!
//!   ┌─ E14-140 DAC Signal Generator ──────────────────────────────────────────┐
//!   │  Channel 0                                                               │
//!   │    Frequency [   5.000] Hz   Phase [   0.00] °                          │
//!   │  Channel 1                                                               │
//!   │    Frequency [  10.000] Hz   Phase [   0.00] °                          │
//!   │  Output                                                                  │
//!   │    Amplitude [   1.00] V   DC Offset [   0.00] V                        │
//!   │  Duration [   5.00] s                                                    │
//!   │                                                                          │
//!   │  [Tab/Shift-Tab] field  [↑↓] value  [Enter] Start  [Esc/Q] Quit         │
//!   └──────────────────────────────────────────────────────────────────────────┘
//!
//! We use only `crossterm` (a single pure-Rust dependency with no system libs).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};

use crate::signal_generator::{generate, GeneratorParams};

// ── Field definitions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    FreqCh0,
    PhaseCh0,
    FreqCh1,
    PhaseCh1,
    Amplitude,
    Offset,
    Duration,
}

const FIELDS: &[Field] = &[
    Field::FreqCh0,
    Field::PhaseCh0,
    Field::FreqCh1,
    Field::PhaseCh1,
    Field::Amplitude,
    Field::Offset,
    Field::Duration,
];

// ── UI State ──────────────────────────────────────────────────────────────────

struct State {
    freq_ch0_hz:     f64,
    phase_ch0_deg:   f64,
    freq_ch1_hz:     f64,
    phase_ch1_deg:   f64,
    amplitude_volts: f64,
    offset_volts:    f64,
    duration_sec:    f64,
    focused:         usize,
    running:         bool,
    status:          String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            freq_ch0_hz:     5.0,
            phase_ch0_deg:   0.0,
            freq_ch1_hz:     10.0,
            phase_ch1_deg:   0.0,
            amplitude_volts: 1.0,
            offset_volts:    0.0,
            duration_sec:    5.0,
            focused:         0,
            running:         false,
            status:          String::new(),
        }
    }
}

impl State {
    fn focused_field(&self) -> Field {
        FIELDS[self.focused]
    }

    fn field_value(&self, f: Field) -> f64 {
        match f {
            Field::FreqCh0   => self.freq_ch0_hz,
            Field::PhaseCh0  => self.phase_ch0_deg,
            Field::FreqCh1   => self.freq_ch1_hz,
            Field::PhaseCh1  => self.phase_ch1_deg,
            Field::Amplitude => self.amplitude_volts,
            Field::Offset    => self.offset_volts,
            Field::Duration  => self.duration_sec,
        }
    }

    fn set_field_value(&mut self, f: Field, v: f64) {
        match f {
            Field::FreqCh0   => self.freq_ch0_hz    = v,
            Field::PhaseCh0  => self.phase_ch0_deg  = v,
            Field::FreqCh1   => self.freq_ch1_hz    = v,
            Field::PhaseCh1  => self.phase_ch1_deg  = v,
            Field::Amplitude => self.amplitude_volts= v,
            Field::Offset    => self.offset_volts   = v,
            Field::Duration  => self.duration_sec   = v,
        }
    }

    fn step_for(f: Field) -> f64 {
        match f {
            Field::FreqCh0 | Field::FreqCh1 => 1.0,
            Field::PhaseCh0 | Field::PhaseCh1 => 1.0,
            Field::Amplitude => 0.1,
            Field::Offset    => 0.1,
            Field::Duration  => 1.0,
        }
    }

    fn clamp(f: Field, v: f64) -> f64 {
        match f {
            Field::FreqCh0 | Field::FreqCh1 => v.max(0.0).min(1_000_000.0),
            Field::PhaseCh0 | Field::PhaseCh1 => v.max(-360.0).min(360.0),
            Field::Amplitude => v.max(0.0).min(10.0),
            Field::Offset    => v.max(-10.0).min(10.0),
            Field::Duration  => v.max(0.1).min(3600.0),
        }
    }

    fn increment(&mut self, delta: f64) {
        let f = self.focused_field();
        let v = Self::clamp(f, self.field_value(f) + delta * Self::step_for(f));
        self.set_field_value(f, v);
    }

    fn params(&self) -> GeneratorParams {
        GeneratorParams {
            duration_sec:    self.duration_sec,
            freq_ch0_hz:     self.freq_ch0_hz,
            freq_ch1_hz:     self.freq_ch1_hz,
            phase_ch0_deg:   self.phase_ch0_deg,
            phase_ch1_deg:   self.phase_ch1_deg,
            amplitude_volts: self.amplitude_volts,
            offset_volts:    self.offset_volts,
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

const WIDTH: u16 = 72;

fn render(w: &mut impl Write, s: &State) -> io::Result<()> {
    queue!(w, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    // ── Title ──
    bold(w)?;
    writeln!(w, " E14-140 DAC Signal Generator")?;
    reset(w)?;
    writeln!(w, " {}", "─".repeat(WIDTH as usize))?;

    // ── Channel 0 ──
    section(w, " Channel 0")?;
    write!(w, "   Frequency  ")?;
    value_field(w, s, Field::FreqCh0,  " Hz")?;
    write!(w, "   Phase  ")?;
    value_field(w, s, Field::PhaseCh0, " °")?;
    writeln!(w)?;

    // ── Channel 1 ──
    section(w, " Channel 1")?;
    write!(w, "   Frequency  ")?;
    value_field(w, s, Field::FreqCh1,  " Hz")?;
    write!(w, "   Phase  ")?;
    value_field(w, s, Field::PhaseCh1, " °")?;
    writeln!(w)?;

    // ── Output ──
    section(w, " Output")?;
    write!(w, "   Amplitude  ")?;
    value_field(w, s, Field::Amplitude, " V")?;
    write!(w, "   DC Offset  ")?;
    value_field(w, s, Field::Offset,    " V")?;
    writeln!(w)?;

    // ── Duration ──
    writeln!(w)?;
    write!(w, "   Duration  ")?;
    value_field(w, s, Field::Duration, " s")?;
    writeln!(w)?;

    // ── Status ──
    writeln!(w)?;
    if s.running {
        queue!(w, SetForegroundColor(Color::Yellow))?;
        write!(w, "   ⟳  Generating…")?;
        reset(w)?;
    } else if !s.status.is_empty() {
        if s.status.starts_with("Error") || s.status.starts_with("err") {
            queue!(w, SetForegroundColor(Color::Red))?;
        } else {
            queue!(w, SetForegroundColor(Color::Green))?;
        }
        write!(w, "   {}", s.status)?;
        reset(w)?;
    }
    writeln!(w)?;

    // ── Help bar ──
    writeln!(w, "\n {}", "─".repeat(WIDTH as usize))?;
    dim(w)?;
    let start_hint = if s.running { "[Esc] Abort" } else { "[Enter] Start" };
    write!(w, " [Tab/Shift-Tab] field  [↑↓] adjust  {start_hint}  [Q] Quit")?;
    reset(w)?;

    w.flush()
}

fn section(w: &mut impl Write, label: &str) -> io::Result<()> {
    bold(w)?;
    write!(w, "\n{label}\n")?;
    reset(w)
}

fn value_field(w: &mut impl Write, s: &State, f: Field, suffix: &str) -> io::Result<()> {
    let v   = s.field_value(f);
    let txt = format!("{v:>9.3}");
    if s.focused_field() == f {
        queue!(w, SetAttribute(Attribute::Reverse))?;
        write!(w, "{txt}")?;
        reset(w)?;
    } else {
        write!(w, "{txt}")?;
    }
    write!(w, "{suffix}")
}

fn bold(w: &mut impl Write) -> io::Result<()> {
    queue!(w, SetAttribute(Attribute::Bold))
}
fn dim(w: &mut impl Write) -> io::Result<()> {
    queue!(w, SetAttribute(Attribute::Dim))
}
fn reset(w: &mut impl Write) -> io::Result<()> {
    queue!(w, ResetColor, SetAttribute(Attribute::Reset))
}

// ── Main TUI loop ─────────────────────────────────────────────────────────────

/// Enter raw mode, run the interactive TUI, restore terminal on exit.
pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = run_inner(&mut stdout);

    execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show)?;
    terminal::disable_raw_mode()?;
    result
}

fn run_inner(w: &mut impl Write) -> io::Result<()> {
    let mut state = State::default();
    let stop_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // Channel used to receive generation results from background thread.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

    render(w, &state)?;

    loop {
        // ── Poll for generation-complete messages ──
        if let Ok(result) = rx.try_recv() {
            state.running = false;
            stop_flag.store(false, Ordering::Relaxed);
            state.status = match result {
                Ok(())   => "Generation complete.".into(),
                Err(msg) => format!("Error: {msg}"),
            };
            render(w, &state)?;
        }

        // ── Input ──
        if event::poll(std::time::Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(KeyEvent { code, modifiers, kind: event::KeyEventKind::Press, .. }) => {
                    match code {
                        // Quit
                        KeyCode::Char('q') | KeyCode::Char('Q')
                        if !state.running => break,
                        KeyCode::Esc if !state.running => break,

                        // Abort running generation
                        KeyCode::Esc if state.running => {
                            stop_flag.store(true, Ordering::Relaxed);
                        }

                        // Navigate fields
                        KeyCode::Tab => {
                            state.focused = (state.focused + 1) % FIELDS.len();
                        }
                        KeyCode::BackTab => {
                            state.focused =
                                (state.focused + FIELDS.len() - 1) % FIELDS.len();
                        }

                        // Adjust value
                        KeyCode::Up => {
                            if !state.running { state.increment(1.0); }
                        }
                        KeyCode::Down => {
                            if !state.running { state.increment(-1.0); }
                        }
                        // Shift+Up/Down for finer control (0.1× step)
                        KeyCode::Char('+') => {
                            if !state.running { state.increment(10.0); }
                        }
                        KeyCode::Char('-') => {
                            if !state.running { state.increment(-10.0); }
                        }

                        // Start generation
                        KeyCode::Enter if !state.running => {
                            // Validate
                            let params = state.params();
                            if let Err(e) = params.validate() {
                                state.status = format!("Error: {e}");
                            } else {
                                state.running = true;
                                state.status.clear();
                                stop_flag.store(false, Ordering::Relaxed);

                                let tx2   = tx.clone();
                                let sf2   = Arc::clone(&stop_flag);
                                thread::spawn(move || {
                                    let res = generate(&params, &sf2);
                                    let _ = tx2.send(res);
                                });
                            }
                        }

                        _ => {}
                    }

                    // Ctrl-C always exits
                    if modifiers == KeyModifiers::CONTROL
                        && code == KeyCode::Char('c')
                    {
                        stop_flag.store(true, Ordering::Relaxed);
                        break;
                    }

                    render(w, &state)?;
                }
                Event::Resize(..) => render(w, &state)?,
                _ => {}
            }
        }
    }

    // If still running, request stop and wait for the thread to drain.
    if state.running {
        stop_flag.store(true, Ordering::Relaxed);
        let _ = rx.recv();
    }

    Ok(())
}