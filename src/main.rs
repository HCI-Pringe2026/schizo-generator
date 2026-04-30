//! E14-140 DAC Signal Generator — Rust rewrite.
//!
//! Run with no arguments for the interactive TUI.
//! Run with arguments for headless CLI mode:
//!
//!   lcard_dac_signal_generator <duration_s> <freq0_hz> <freq1_hz>
//!                               [phase0_deg] [phase1_deg]
//!                               [amplitude_v] [offset_v]

mod lusbapi;
mod signal_generator;
mod ui;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use signal_generator::GeneratorParams;

// ── Global Ctrl-C hook storage ────────────────────────────────────────────────

static CTRLC_HOOK: OnceLock<Mutex<Box<dyn Fn() + Send>>> = OnceLock::new();

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        // ── Interactive TUI mode ──
        if let Err(e) = ui::run() {
            eprintln!("TUI error: {e}");
            std::process::exit(1);
        }
    } else {
        // ── Headless CLI mode ──
        let params = match parse_cli(&args) {
            Ok(p)    => p,
            Err(msg) => {
                eprintln!("{msg}");
                print_usage();
                std::process::exit(2);
            }
        };

        let stop = Arc::new(AtomicBool::new(false));

        {
            let stop2 = Arc::clone(&stop);
            install_ctrlc(move || stop2.store(true, Ordering::Relaxed));
        }

        match signal_generator::generate(&params, &stop) {
            Ok(())   => {}
            Err(msg) => {
                eprintln!("Error: {msg}");
                std::process::exit(1);
            }
        }
    }
}

// ── CLI argument parsing ──────────────────────────────────────────────────────

fn parse_cli(args: &[String]) -> Result<GeneratorParams, String> {
    fn f(s: &str, name: &str) -> Result<f64, String> {
        s.parse::<f64>()
            .map_err(|_| format!("Invalid value for {name}: '{s}'"))
    }
    if args.len() < 3 {
        return Err("At least 3 arguments required.".into());
    }
    Ok(GeneratorParams {
        duration_sec:    f(&args[0], "duration_s")?,
        freq_ch0_hz:     f(&args[1], "freq0_hz")?,
        freq_ch1_hz:     f(&args[2], "freq1_hz")?,
        phase_ch0_deg:   args.get(3).map(|s| f(s, "phase0_deg")).transpose()?.unwrap_or(0.0),
        phase_ch1_deg:   args.get(4).map(|s| f(s, "phase1_deg")).transpose()?.unwrap_or(0.0),
        amplitude_volts: args.get(5).map(|s| f(s, "amplitude_v")).transpose()?.unwrap_or(1.0),
        offset_volts:    args.get(6).map(|s| f(s, "offset_v")).transpose()?.unwrap_or(0.0),
    })
}

fn print_usage() {
    eprintln!(
        "\nUsage:\n\
         \n  lcard_dac_signal_generator                        (interactive TUI)\n\
         \n  lcard_dac_signal_generator <duration_s> <freq0_hz> <freq1_hz>\n\
                                      [phase0_deg=0] [phase1_deg=0]\n\
                                      [amplitude_v=1.0] [offset_v=0.0]\n\
         \nExample:\n\
          lcard_dac_signal_generator 10 50 100 0 90 1.5 0.5\n"
    );
}

// ── Ctrl-C / console-break handling ──────────────────────────────────────────

fn install_ctrlc(cb: impl Fn() + Send + 'static) {
    let _ = CTRLC_HOOK.set(Mutex::new(Box::new(cb)));

    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_C_EVENT};

        unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
            if ctrl_type == CTRL_C_EVENT {
                if let Some(m) = CTRLC_HOOK.get() {
                    if let Ok(f) = m.lock() { f(); }
                }
            }
            BOOL(0)
        }
        let _ = SetConsoleCtrlHandler(Some(handler), true);
    }
}
