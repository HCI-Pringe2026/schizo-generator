//! Terminal-based UI replacing the original Qt `MainWindow`.
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use crossterm::{
    cursor, event::{self, Event, KeyCode, KeyEvent, KeyModifiers}, execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor}, terminal::{self, ClearType},
};
use crate::signal_generator::{generate, generate_cycles, GeneratorParams, MultiCycleParams, CycleStep};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field { FreqCh0, PhaseCh0, FreqCh1, PhaseCh1, Amplitude, Offset, Duration, NumCycles, PauseSec }
const FIELDS: &[Field] = &[Field::FreqCh0, Field::PhaseCh0, Field::FreqCh1, Field::PhaseCh1, Field::Amplitude, Field::Offset, Field::Duration, Field::NumCycles, Field::PauseSec];

struct State {
    freq_ch0_hz: f64, phase_ch0_deg: f64, freq_ch1_hz: f64, phase_ch1_deg: f64,
    amplitude_volts: f64, offset_volts: f64, duration_sec: f64,
    num_cycles: usize, pause_sec: f64,
    cycle_frequencies: Vec<(f64, f64)>,
    focused: usize, running: bool, status: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            freq_ch0_hz: 5.0, phase_ch0_deg: 0.0, freq_ch1_hz: 10.0, phase_ch1_deg: 0.0,
            amplitude_volts: 1.0, offset_volts: 0.0, duration_sec: 5.0,
            num_cycles: 1, pause_sec: 1.0,
            cycle_frequencies: vec![(5.0, 10.0), (15.0, 20.0), (25.0, 30.0), (5.0, 10.0)],
            focused: 0, running: false, status: String::new(),
        }
    }
}

impl State {
    fn focused_field(&self) -> Field { FIELDS[self.focused] }
    fn field_value(&self, f: Field) -> f64 {
        match f {
            Field::FreqCh0 => self.freq_ch0_hz, Field::PhaseCh0 => self.phase_ch0_deg,
            Field::FreqCh1 => self.freq_ch1_hz, Field::PhaseCh1 => self.phase_ch1_deg,
            Field::Amplitude => self.amplitude_volts, Field::Offset => self.offset_volts,
            Field::Duration => self.duration_sec, Field::PauseSec => self.pause_sec,
            Field::NumCycles => self.num_cycles as f64,
        }
    }
    fn set_field_value(&mut self, f: Field, v: f64) {
        match f {
            Field::FreqCh0 => self.freq_ch0_hz = v, Field::PhaseCh0 => self.phase_ch0_deg = v,
            Field::FreqCh1 => self.freq_ch1_hz = v, Field::PhaseCh1 => self.phase_ch1_deg = v,
            Field::Amplitude => self.amplitude_volts = v, Field::Offset => self.offset_volts = v,
            Field::Duration => self.duration_sec = v, Field::PauseSec => self.pause_sec = v,
            Field::NumCycles => self.num_cycles = v.clamp(1.0, 10.0) as usize,
        }
    }
    fn step_for(f: Field) -> f64 { match f { Field::FreqCh0 | Field::FreqCh1 => 1.0, Field::PhaseCh0 | Field::PhaseCh1 => 1.0, Field::Amplitude | Field::Offset | Field::PauseSec => 0.1, Field::Duration => 1.0, Field::NumCycles => 1.0 } }
    fn clamp(f: Field, v: f64) -> f64 { match f { Field::FreqCh0 | Field::FreqCh1 => v.max(0.0).min(1_000_000.0), Field::PhaseCh0 | Field::PhaseCh1 => v.max(-360.0).min(360.0), Field::Amplitude => v.max(0.0).min(10.0), Field::Offset => v.max(-10.0).min(10.0), Field::Duration | Field::PauseSec => v.max(0.1).min(3600.0), Field::NumCycles => v.max(1.0).min(10.0) } }
    fn increment(&mut self, delta: f64) { let f = self.focused_field(); self.set_field_value(f, Self::clamp(f, self.field_value(f) + delta * Self::step_for(f))); }
    fn params(&self) -> GeneratorParams {
        GeneratorParams { duration_sec: self.duration_sec, freq_ch0_hz: self.freq_ch0_hz, freq_ch1_hz: self.freq_ch1_hz, phase_ch0_deg: self.phase_ch0_deg, phase_ch1_deg: self.phase_ch1_deg, amplitude_volts: self.amplitude_volts, offset_volts: self.offset_volts }
    }
    fn to_multi_cycle_params(&self) -> MultiCycleParams {
        MultiCycleParams {
            steps: self.cycle_frequencies.iter().take(self.num_cycles).map(|&(f0, f1)| CycleStep { freq_ch0_hz: f0, freq_ch1_hz: f1 }).collect(),
            duration_sec_per_step: self.duration_sec, pause_sec_between_steps: self.pause_sec,
            amplitude_volts: self.amplitude_volts, offset_volts: self.offset_volts,
            phase_ch0_deg: self.phase_ch0_deg, phase_ch1_deg: self.phase_ch1_deg,
        }
    }
}

const WIDTH: u16 = 72;
fn render(w: &mut impl Write, s: &State) -> io::Result<()> {
    queue!(w, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    bold(w)?; writeln!(w, " E14-140 DAC Signal Generator ")?; reset(w)?;
    writeln!(w, " {}", "─".repeat(WIDTH as usize))?;
    section(w, " Channel 0 ")?; write!(w, "   Frequency   ")?; value_field(w, s, Field::FreqCh0, " Hz ")?; write!(w, "   Phase   ")?; value_field(w, s, Field::PhaseCh0, " ° ")?; writeln!(w)?;
    section(w, " Channel 1 ")?; write!(w, "   Frequency   ")?; value_field(w, s, Field::FreqCh1, " Hz ")?; write!(w, "   Phase   ")?; value_field(w, s, Field::PhaseCh1, " ° ")?; writeln!(w)?;
    section(w, " Output ")?; write!(w, "   Amplitude   ")?; value_field(w, s, Field::Amplitude, " V ")?; write!(w, "   DC Offset   ")?; value_field(w, s, Field::Offset, " V ")?; writeln!(w)?;
    writeln!(w)?; write!(w, "   Duration   ")?; value_field(w, s, Field::Duration, " s ")?; writeln!(w)?;
    section(w, " Cycles ")?; write!(w, "   Count:   ")?; value_field_usize(w, s, s.num_cycles, "")?; write!(w, "   Pause:   ")?; value_field(w, s, Field::PauseSec, " s ")?; writeln!(w)?;
    for i in 0..s.num_cycles.min(s.cycle_frequencies.len()) {
        let (f0, f1) = s.cycle_frequencies[i];
        writeln!(w, "   Cycle {}:  F0={:6.1} Hz  F1={:6.1} Hz", i + 1, f0, f1)?;
    }
    writeln!(w)?;
    if s.running { queue!(w, SetForegroundColor(Color::Yellow))?; write!(w, "   ⟳  Generating… ")?; reset(w)?; } 
    else if !s.status.is_empty() { if s.status.starts_with("Error ") { queue!(w, SetForegroundColor(Color::Red))?; } else { queue!(w, SetForegroundColor(Color::Green))?; } write!(w, "   {} ", s.status)?; reset(w)?; }
    writeln!(w)?; writeln!(w, " {}", "─".repeat(WIDTH as usize))?; dim(w)?;
    let start_hint = if s.running { "[Esc] Abort " } else { "[Enter] Start " };
    write!(w, " [Tab/Shift-Tab] field  [↑↓] adjust  {start_hint}  [Q] Quit ")?; reset(w)?; w.flush()
}

fn section(w: &mut impl Write, label: &str) -> io::Result<()> { bold(w)?; write!(w, "\n{label}\n")?; reset(w) }
fn value_field(w: &mut impl Write, s: &State, f: Field, suffix: &str) -> io::Result<()> {
    let v = s.field_value(f); let txt = format!("{v:>9.3}");
    if s.focused_field() == f { queue!(w, SetAttribute(Attribute::Reverse))?; write!(w, "{txt} ")?; reset(w)?; } else { write!(w, "{txt} ")?; }
    write!(w, "{suffix} ")
}
fn value_field_usize(w: &mut impl Write, _s: &State, val: usize, suffix: &str) -> io::Result<()> {
    let txt = format!("{val:>9}"); queue!(w, SetAttribute(Attribute::Reverse))?; write!(w, "{txt} ")?; reset(w)?; write!(w, "{suffix} ")
}
fn bold(w: &mut impl Write) -> io::Result<()> { queue!(w, SetAttribute(Attribute::Bold)) }
fn dim(w: &mut impl Write) -> io::Result<()> { queue!(w, SetAttribute(Attribute::Dim)) }
fn reset(w: &mut impl Write) -> io::Result<()> { queue!(w, ResetColor, SetAttribute(Attribute::Reset)) }

pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
    let result = run_inner(&mut stdout);
    execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show)?;
    terminal::disable_raw_mode()?; result
}

fn run_inner(w: &mut impl Write) -> io::Result<()> {
    let mut state = State::default();
    let stop_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    render(w, &state)?;

    loop {
        if let Ok(result) = rx.try_recv() {
            state.running = false; stop_flag.store(false, Ordering::Relaxed);
            state.status = match result { Ok(()) => "Generation complete.".into(), Err(msg) => format!("Error: {msg}") };
            render(w, &state)?;
        }
        if event::poll(std::time::Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(KeyEvent { code, modifiers, kind: event::KeyEventKind::Press, .. }) => {
                    match code {
                        KeyCode::Char('q') | KeyCode::Char('Q') if !state.running => break,
                        KeyCode::Esc if !state.running => break,
                        KeyCode::Esc if state.running => { stop_flag.store(true, Ordering::Relaxed); }
                        KeyCode::Tab => { state.focused = (state.focused + 1) % FIELDS.len(); }
                        KeyCode::BackTab => { state.focused = (state.focused + FIELDS.len() - 1) % FIELDS.len(); }
                        KeyCode::Up => { if !state.running { state.increment(1.0); } }
                        KeyCode::Down => { if !state.running { state.increment(-1.0); } }
                        KeyCode::Char('+') => { if !state.running { state.increment(10.0); } }
                        KeyCode::Char('-') => { if !state.running { state.increment(-10.0); } }
                        KeyCode::Enter if !state.running => {
                            if state.num_cycles == 1 {
                                let params = state.params();
                                if let Err(e) = params.validate() { state.status = format!("Error: {e}"); }
                                else { state.running = true; state.status.clear(); stop_flag.store(false, Ordering::Relaxed); let tx2 = tx.clone(); let sf2 = Arc::clone(&stop_flag); thread::spawn(move || { let _ = tx2.send(generate(&params, &sf2)); }); }
                            } else {
                                let params = state.to_multi_cycle_params();
                                if let Err(e) = params.validate() { state.status = format!("Error: {e}"); }
                                else { state.running = true; state.status.clear(); stop_flag.store(false, Ordering::Relaxed); let tx2 = tx.clone(); let sf2 = Arc::clone(&stop_flag); thread::spawn(move || { let _ = tx2.send(generate_cycles(&params, &sf2)); }); }
                            }
                        }
                        _ => {}
                    }
                    if modifiers == KeyModifiers::CONTROL && code == KeyCode::Char('c') { stop_flag.store(true, Ordering::Relaxed); break; }
                    render(w, &state)?;
                }
                Event::Resize(..) => render(w, &state)?,
                _ => {}
            }
        }
    }
    if state.running { stop_flag.store(true, Ordering::Relaxed); let _ = rx.recv(); }
    Ok(())
}
