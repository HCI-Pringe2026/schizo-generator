//! Core signal generation logic for the E14-140 DAC.
//!
//! Mirrors `SignalGenerator.cpp` from the original Qt/C++ project,
//! rewritten in idiomatic Rust with no unsafe outside the FFI boundary.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::Win32::System::IO::OVERLAPPED;
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};

use crate::lusbapi::{Lusbapi, DAC_PARS_E140, IO_REQUEST_LUSBAPI, SHORT};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Half the double-buffer size in DAC word pairs.
const DATA_STEP_WORDS: usize = 32_768;

/// DAC bipolar full scale (±10 V).
const DAC_FULL_SCALE_VOLTS: f64 = 10.0;

// ── Parameters ────────────────────────────────────────────────────────────────

/// All parameters needed to run one generation session.
#[derive(Debug, Clone)]
pub struct GeneratorParams {
    pub duration_sec:    f64,
    pub freq_ch0_hz:     f64,
    pub freq_ch1_hz:     f64,
    pub phase_ch0_deg:   f64,
    pub phase_ch1_deg:   f64,
    pub amplitude_volts: f64,
    pub offset_volts:    f64,
}

impl GeneratorParams {
    /// Validate parameter ranges.
    pub fn validate(&self) -> Result<(), String> {
        if self.duration_sec <= 0.0 {
            return Err("duration_sec must be > 0".into());
        }
        if self.freq_ch0_hz < 0.0 || self.freq_ch1_hz < 0.0 {
            return Err("Frequencies must be >= 0 Hz".into());
        }
        if self.amplitude_volts < 0.0 {
            return Err("amplitude_volts must be >= 0".into());
        }
        if self.amplitude_volts + self.offset_volts.abs() > DAC_FULL_SCALE_VOLTS {
            return Err(format!(
                "amplitude ({:.4} V) + |offset ({:.4} V)| exceeds DAC full scale ({DAC_FULL_SCALE_VOLTS} V)",
                self.amplitude_volts, self.offset_volts,
            ));
        }
        Ok(())
    }
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Open the device, configure the DAC, stream the signal, then clean up.
///
/// `stop_flag` is polled after every completed overlapped request; set it
/// from another thread to request early termination.
pub fn generate(params: &GeneratorParams, stop_flag: &Arc<AtomicBool>) -> Result<(), String> {
    params.validate()?;

    let mut device = Lusbapi::open()?;

    let requested_rate_hz = select_requested_rate(params.freq_ch0_hz, params.freq_ch1_hz);
    let actual_rate_hz    = configure_dac(&mut device, requested_rate_hz)?;

    let total_pairs = (params.duration_sec * actual_rate_hz + 0.5) as u64;

    print_run_info(params, requested_rate_hz, actual_rate_hz, total_pairs, &device);

    stream_signal(&mut device, params, actual_rate_hz, total_pairs, stop_flag)?;

    if !device.stop_dac() {
        return Err("STOP_DAC() after streaming failed".into());
    }

    println!("Done.");
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn select_requested_rate(freq0_hz: f64, freq1_hz: f64) -> f64 {
    let mut rate = 25_000.0_f64;
    let max_freq = freq0_hz.max(freq1_hz);
    if max_freq > 0.0 {
        let by_quality = max_freq * 32.0;
        if by_quality > rate {
            rate = by_quality;
        }
    }
    rate.min(200_000.0)
}

fn configure_dac(device: &mut Lusbapi, requested_rate_hz: f64) -> Result<f64, String> {
    if !device.stop_dac() {
        return Err("STOP_DAC() before SET_DAC_PARS() failed".into());
    }

    let mut pars = DAC_PARS_E140 {
        SyncWithADC:  0,
        SetZeroOnStop: 1,
        DacRate:      requested_rate_hz / 1000.0, // kHz
    };

    if !device.set_dac_pars(&mut pars) {
        return Err("SET_DAC_PARS() failed".into());
    }

    // The DLL adjusts DacRate to the nearest achievable value.
    device.dac_pars = pars;
    let actual_rate_khz = pars.DacRate;
    Ok(actual_rate_khz * 1000.0)
}

/// Double-buffered async streaming loop.
fn stream_signal(
    device:      &mut Lusbapi,
    params:      &GeneratorParams,
    rate_hz:     f64,
    total_pairs: u64,
    stop_flag:   &Arc<AtomicBool>,
) -> Result<(), String> {
    // Allocate write buffer: two halves, each DATA_STEP_WORDS words
    // interleaved as [ch0, ch1, ch0, ch1, …]
    let mut write_buf: Vec<SHORT> = vec![0; 2 * DATA_STEP_WORDS];

    // Create two Windows events for overlapped I/O
    let events: [HANDLE; 2] = unsafe {
        [
            CreateEventW(None, false, false, None)
                .map_err(|e| format!("CreateEvent[0] failed: {e}"))?,
            CreateEventW(None, false, false, None)
                .map_err(|e| format!("CreateEvent[1] failed: {e}"))?,
        ]
    };

    // Safety: OVERLAPPED must remain pinned for the lifetime of the requests.
    let mut overlapped: [OVERLAPPED; 2] = [
        unsafe { std::mem::zeroed() },
        unsafe { std::mem::zeroed() },
    ];
    overlapped[0].hEvent = events[0];
    overlapped[1].hEvent = events[1];

    let timeout_ms =
        (1000.0 * DATA_STEP_WORDS as f64 / rate_hz + 1000.0) as u32;

    // SAFETY: The buffer halves are distinct, non-overlapping slices.
    let (buf0, buf1) = write_buf.split_at_mut(DATA_STEP_WORDS);

    let mut requests: [IO_REQUEST_LUSBAPI; 2] = [
        IO_REQUEST_LUSBAPI {
            Buffer:              buf0.as_mut_ptr(),
            NumberOfWordsToPass: DATA_STEP_WORDS as u32,
            NumberOfWordsPassed: 0,
            Overlapped:          &mut overlapped[0],
            TimeOut:             timeout_ms,
        },
        IO_REQUEST_LUSBAPI {
            Buffer:              buf1.as_mut_ptr(),
            NumberOfWordsToPass: DATA_STEP_WORDS as u32,
            NumberOfWordsPassed: 0,
            Overlapped:          &mut overlapped[1],
            TimeOut:             timeout_ms,
        },
    ];

    // Streaming state
    let mut phase0 = normalise_phase(params.phase_ch0_deg.to_radians());
    let mut phase1 = normalise_phase(params.phase_ch1_deg.to_radians());
    let mut pairs_generated: u64 = 0;

    // Fill both buffers before we start
    fill_interleaved(
        requests[0].Buffer,
        DATA_STEP_WORDS,
        rate_hz, params,
        &mut phase0, &mut phase1,
        &mut pairs_generated, total_pairs,
        device,
    );
    fill_interleaved(
        requests[1].Buffer,
        DATA_STEP_WORDS,
        rate_hz, params,
        &mut phase0, &mut phase1,
        &mut pairs_generated, total_pairs,
        device,
    );

    // Issue first request, start DAC, issue second request
    if !device.write_data(&mut requests[0]) {
        return Err("Initial WriteData(requests[0]) failed".into());
    }
    let mut active = [true, true];

    if !device.start_dac() {
        return Err("START_DAC() failed".into());
    }

    if !device.write_data(&mut requests[1]) {
        return Err("Initial WriteData(requests[1]) failed".into());
    }

    let mut no_more_data = pairs_generated >= total_pairs;

    // Main loop
    while active[0] || active[1] {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let wait_handles = [events[0], events[1]];
        let signaled = unsafe {
            WaitForMultipleObjects(&wait_handles, false, INFINITE)
        };

        let idx = match signaled {
            WAIT_OBJECT_0     => 0usize,
            s if s.0 == WAIT_OBJECT_0.0 + 1 => 1,
            _ => return Err("WaitForMultipleObjects() failed".into()),
        };

        // Retrieve overlapped result (busy-wait on ERROR_IO_INCOMPLETE)
        wait_overlapped_completed(device.module_handle, &mut overlapped[idx])?;
        active[idx] = false;

        if !no_more_data {
            fill_interleaved(
                requests[idx].Buffer,
                DATA_STEP_WORDS,
                rate_hz, params,
                &mut phase0, &mut phase1,
                &mut pairs_generated, total_pairs,
                device,
            );
            no_more_data = pairs_generated >= total_pairs;

            if !device.write_data(&mut requests[idx]) {
                return Err("WriteData() in streaming loop failed".into());
            }
            active[idx] = true;
        }
    }

    // Close event handles
    unsafe {
        let _ = CloseHandle(events[0]);
        let _ = CloseHandle(events[1]);
    }

    Ok(())
}

/// Spin-wait variant of `GetOverlappedResult` (mirrors the C++ original).
fn wait_overlapped_completed(
    handle: HANDLE,
    overlapped: &mut OVERLAPPED,
) -> Result<u32, String> {
    use windows::Win32::System::IO::GetOverlappedResult;
    use windows::Win32::System::Threading::Sleep;

    // ERROR_IO_INCOMPLETE = 996
    const ERROR_IO_INCOMPLETE: u32 = 996;

    loop {
        let mut bytes: u32 = 0;
        let result = unsafe { GetOverlappedResult(handle, overlapped, &mut bytes, false) };
        match result {
            Ok(()) => return Ok(bytes / std::mem::size_of::<SHORT>() as u32),
            Err(e) => {
                let code = e.code().0 as u32 & 0xFFFF;
                if code != ERROR_IO_INCOMPLETE {
                    return Err(format!("GetOverlappedResult() failed: {e}"));
                }
                // IO still pending — yield briefly and retry
                unsafe { Sleep(1) };
            }
        }
    }
}

// ── Signal synthesis ──────────────────────────────────────────────────────────

/// Fill one half-buffer with interleaved [ch0, ch1] pairs.
///
/// # Safety
/// `buf` must point to at least `words` valid `SHORT` slots.
fn fill_interleaved(
    buf:             *mut SHORT,
    words:           usize,
    rate_hz:         f64,
    params:          &GeneratorParams,
    phase0:          &mut f64,
    phase1:          &mut f64,
    pairs_generated: &mut u64,
    total_pairs:     u64,
    device:          &Lusbapi,
) {
    let words = if words & 1 != 0 { words - 1 } else { words };
    let pairs = words / 2;
    let dphi0 = 2.0 * PI * params.freq_ch0_hz / rate_hz;
    let dphi1 = 2.0 * PI * params.freq_ch1_hz / rate_hz;

    for i in 0..pairs {
        let (s0, s1) = if *pairs_generated < total_pairs {
            let v0 = params.amplitude_volts * phase0.sin() + params.offset_volts;
            let v1 = params.amplitude_volts * phase1.sin() + params.offset_volts;

            *phase0 = normalise_phase(*phase0 + dphi0);
            *phase1 = normalise_phase(*phase1 + dphi1);
            *pairs_generated += 1;

            (
                make_voltage_sample(v0, 0, device),
                make_voltage_sample(v1, 1, device),
            )
        } else {
            (
                make_voltage_sample(params.offset_volts, 0, device),
                make_voltage_sample(params.offset_volts, 1, device),
            )
        };

        unsafe {
            *buf.add(2 * i)     = s0;
            *buf.add(2 * i + 1) = s1;
        }
    }
}

fn normalise_phase(mut p: f64) -> f64 {
    let two_pi = 2.0 * PI;
    p %= two_pi;
    if p < 0.0 {
        p += two_pi;
    }
    p
}

fn make_voltage_sample(voltage: f64, channel: usize, device: &Lusbapi) -> SHORT {
    let normalized = (voltage / DAC_FULL_SCALE_VOLTS).clamp(-1.0, 1.0);
    let raw = normalized * 32767.0;
    let a   = device.module_description.dac_offset_calibration(channel);
    let b   = device.module_description.dac_scale_calibration(channel);
    let corrected = (raw + a) * b;
    clamp_to_short(corrected)
}

fn clamp_to_short(x: f64) -> SHORT {
    let x = x.clamp(-32768.0, 32767.0);
    if x >= 0.0 { (x + 0.5) as SHORT } else { (x - 0.5) as SHORT }
}

// ── Print helper ──────────────────────────────────────────────────────────────

fn print_run_info(
    params:          &GeneratorParams,
    requested_rate:  f64,
    actual_rate:     f64,
    total_pairs:     u64,
    device:          &Lusbapi,
) {
    let desc = &device.module_description;
    let sn   = desc.serial_number();
    let usb  = if device.usb_speed != 0 { "High-Speed (480 Mbit/s)" } else { "Full-Speed (12 Mbit/s)" };

    println!("Module:              E14-140, serial: {}", sn);
    println!("Revision:            {}", desc.revision() as char);
    println!("USB mode:            {usb}");
    println!("Requested DAC rate:  {:.3} kHz", requested_rate / 1000.0);
    println!("Actual DAC rate:     {:.3} kHz", actual_rate    / 1000.0);
    println!("Duration:            {:.3} s",   params.duration_sec);
    println!("Ch0 frequency:       {:.3} Hz",  params.freq_ch0_hz);
    println!("Ch1 frequency:       {:.3} Hz",  params.freq_ch1_hz);
    println!("Ch0 phase:           {:.3}°",    params.phase_ch0_deg);
    println!("Ch1 phase:           {:.3}°",    params.phase_ch1_deg);
    println!("Amplitude:           {:.4} V (peak)",  params.amplitude_volts);
    println!("DC offset:           {:.4} V",         params.offset_volts);
    println!("Peak voltage:        {:.4} V",  params.amplitude_volts + params.offset_volts.abs());
    println!("Total sample pairs:  {total_pairs}");
}
