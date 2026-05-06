//! Core signal generation logic for the E14-140 DAC.
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, Sleep, WaitForMultipleObjects, INFINITE};
use crate::lusbapi::{Lusbapi, DAC_PARS_E140, IO_REQUEST_LUSBAPI, SHORT};

const DATA_STEP_WORDS: usize = 32_768;
const DAC_FULL_SCALE_VOLTS: f64 = 10.0;

#[derive(Debug, Clone)]
pub struct GeneratorParams {
    pub duration_sec: f64,
    pub freq_ch0_hz: f64,
    pub freq_ch1_hz: f64,
    pub phase_ch0_deg: f64,
    pub phase_ch1_deg: f64,
    pub amplitude_volts: f64,
    pub offset_volts: f64,
}

impl GeneratorParams {
    pub fn validate(&self) -> Result<(), String> {
        if self.duration_sec <= 0.0 { return Err("duration_sec must be > 0".into()); }
        if self.freq_ch0_hz < 0.0 || self.freq_ch1_hz < 0.0 { return Err("Frequencies must be >= 0 Hz".into()); }
        if self.amplitude_volts < 0.0 { return Err("amplitude_volts must be >= 0".into()); }
        if self.amplitude_volts + self.offset_volts.abs() > DAC_FULL_SCALE_VOLTS {
            return Err(format!("amplitude ({:.4} V) + |offset ({:.4} V)| exceeds DAC full scale ({DAC_FULL_SCALE_VOLTS} V)", self.amplitude_volts, self.offset_volts));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CycleStep {
    pub freq_ch0_hz: f64,
    pub freq_ch1_hz: f64,
}

#[derive(Debug, Clone)]
pub struct MultiCycleParams {
    pub steps: Vec<CycleStep>,
    pub duration_sec_per_step: f64,
    pub pause_sec_between_steps: f64,
    pub amplitude_volts: f64,
    pub offset_volts: f64,
    pub phase_ch0_deg: f64,
    pub phase_ch1_deg: f64,
}

impl MultiCycleParams {
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() { return Err("At least one cycle step is required".into()); }
        if self.duration_sec_per_step <= 0.0 { return Err("duration_sec_per_step must be > 0".into()); }
        if self.pause_sec_between_steps < 0.0 { return Err("pause_sec_between_steps must be >= 0".into()); }
        if self.amplitude_volts < 0.0 { return Err("amplitude_volts must be >= 0".into()); }
        if self.amplitude_volts + self.offset_volts.abs() > DAC_FULL_SCALE_VOLTS {
            return Err(format!("amplitude + |offset| exceeds {DAC_FULL_SCALE_VOLTS} V"));
        }
        for step in &self.steps {
            if step.freq_ch0_hz < 0.0 || step.freq_ch1_hz < 0.0 { return Err("Frequencies must be >= 0 Hz".into()); }
        }
        Ok(())
    }
}

pub fn generate(params: &GeneratorParams, stop_flag: &Arc<AtomicBool>) -> Result<(), String> {
    params.validate()?;
    let mut device = Lusbapi::open()?;
    let requested_rate_hz = select_requested_rate(params.freq_ch0_hz, params.freq_ch1_hz);
    let actual_rate_hz = configure_dac(&mut device, requested_rate_hz)?;
    let total_pairs = (params.duration_sec * actual_rate_hz + 0.5) as u64;
    print_run_info(params, requested_rate_hz, actual_rate_hz, total_pairs, &device);
    stream_signal(&mut device, params, actual_rate_hz, total_pairs, stop_flag)?;
    if !device.stop_dac() { return Err("STOP_DAC() after streaming failed".into()); }
    println!("Done.");
    Ok(())
}

pub fn generate_cycles(params: &MultiCycleParams, stop_flag: &Arc<AtomicBool>) -> Result<(), String> {
    params.validate()?;
    let num_steps = params.steps.len();
    for i in 0..num_steps {
        if stop_flag.load(Ordering::Relaxed) { println!("Aborted during cycle transition."); return Ok(()); }
        
        let cycle = &params.steps[i];
        let single = GeneratorParams {
            duration_sec: params.duration_sec_per_step,
            freq_ch0_hz: cycle.freq_ch0_hz,
            freq_ch1_hz: cycle.freq_ch1_hz,
            phase_ch0_deg: params.phase_ch0_deg,
            phase_ch1_deg: params.phase_ch1_deg,
            amplitude_volts: params.amplitude_volts,
            offset_volts: params.offset_volts,
        };

        println!("--- Cycle {}/{} (F0: {:.1} Hz, F1: {:.1} Hz) ---", i + 1, num_steps, cycle.freq_ch0_hz, cycle.freq_ch1_hz);
        generate(&single, stop_flag)?;

        if i < num_steps - 1 && params.pause_sec_between_steps > 0.0 {
            println!("Pausing for {:.2} s...", params.pause_sec_between_steps);
            let pause_dur = Duration::from_secs_f64(params.pause_sec_between_steps);
            let start = std::time::Instant::now();
            while start.elapsed() < pause_dur {
                if stop_flag.load(Ordering::Relaxed) { println!("Aborted during pause."); return Ok(()); }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    println!("All cycles completed successfully.");
    Ok(())
}

fn select_requested_rate(freq0_hz: f64, freq1_hz: f64) -> f64 {
    let mut rate = 25_000.0_f64;
    let max_freq = freq0_hz.max(freq1_hz);
    if max_freq > 0.0 { let by_quality = max_freq * 32.0; if by_quality > rate { rate = by_quality; } }
    rate.min(200_000.0)
}

fn configure_dac(device: &mut Lusbapi, requested_rate_hz: f64) -> Result<f64, String> {
    if !device.stop_dac() { return Err("STOP_DAC() before SET_DAC_PARS() failed".into()); }
    let mut pars = DAC_PARS_E140 { SyncWithADC: 0, SetZeroOnStop: 1, DacRate: requested_rate_hz / 1000.0 };
    if !device.set_dac_pars(&mut pars) { return Err("SET_DAC_PARS() failed".into()); }
    device.dac_pars = pars;
    Ok(pars.DacRate * 1000.0)
}

fn stream_signal(device: &mut Lusbapi, params: &GeneratorParams, rate_hz: f64, total_pairs: u64, stop_flag: &Arc<AtomicBool>) -> Result<(), String> {
    let mut write_buf: Vec<SHORT> = vec![0; 2 * DATA_STEP_WORDS];
    let events: [HANDLE; 2] = unsafe {
        [
            CreateEventW(None, false, false, None).map_err(|e| format!("CreateEvent[0] failed: {e}"))?,
            CreateEventW(None, false, false, None).map_err(|e| format!("CreateEvent[1] failed: {e}"))?,
        ]
    };
    let mut overlapped: [OVERLAPPED; 2] = [unsafe { std::mem::zeroed() }, unsafe { std::mem::zeroed() }];
    overlapped[0].hEvent = events[0];
    overlapped[1].hEvent = events[1];
    let timeout_ms = (1000.0 * DATA_STEP_WORDS as f64 / rate_hz + 1000.0) as u32;
    let (buf0, buf1) = write_buf.split_at_mut(DATA_STEP_WORDS);
    let mut requests: [IO_REQUEST_LUSBAPI; 2] = [
        IO_REQUEST_LUSBAPI { Buffer: buf0.as_mut_ptr(), NumberOfWordsToPass: DATA_STEP_WORDS as u32, NumberOfWordsPassed: 0, Overlapped: &mut overlapped[0], TimeOut: timeout_ms },
        IO_REQUEST_LUSBAPI { Buffer: buf1.as_mut_ptr(), NumberOfWordsToPass: DATA_STEP_WORDS as u32, NumberOfWordsPassed: 0, Overlapped: &mut overlapped[1], TimeOut: timeout_ms },
    ];
    let mut phase0 = normalise_phase(params.phase_ch0_deg.to_radians());
    let mut phase1 = normalise_phase(params.phase_ch1_deg.to_radians());
    let mut pairs_generated: u64 = 0;
    fill_interleaved(requests[0].Buffer, DATA_STEP_WORDS, rate_hz, params, &mut phase0, &mut phase1, &mut pairs_generated, total_pairs, device);
    fill_interleaved(requests[1].Buffer, DATA_STEP_WORDS, rate_hz, params, &mut phase0, &mut phase1, &mut pairs_generated, total_pairs, device);
    if !device.write_data(&mut requests[0]) { return Err("Initial WriteData(requests[0]) failed".into()); }
    let mut active = [true, true];
    if !device.start_dac() { return Err("START_DAC() failed".into()); }
    if !device.write_data(&mut requests[1]) { return Err("Initial WriteData(requests[1]) failed".into()); }
    let mut no_more_data = pairs_generated >= total_pairs;

    while active[0] || active[1] {
        if stop_flag.load(Ordering::Relaxed) { break; }
        let wait_handles = [events[0], events[1]];
        let signaled = unsafe { WaitForMultipleObjects(&wait_handles, false, INFINITE) };
        let idx = match signaled {
            WAIT_OBJECT_0 => 0usize,
            s if s.0 == WAIT_OBJECT_0.0 + 1 => 1,
            _ => return Err("WaitForMultipleObjects() failed".into()),
        };
        wait_overlapped_completed(device.module_handle, &mut overlapped[idx])?;
        active[idx] = false;
        if !no_more_data {
            fill_interleaved(requests[idx].Buffer, DATA_STEP_WORDS, rate_hz, params, &mut phase0, &mut phase1, &mut pairs_generated, total_pairs, device);
            no_more_data = pairs_generated >= total_pairs;
            if !device.write_data(&mut requests[idx]) { return Err("WriteData() in streaming loop failed".into()); }
            active[idx] = true;
        }
    }
    unsafe { let _ = CloseHandle(events[0]); let _ = CloseHandle(events[1]); }
    Ok(())
}

fn wait_overlapped_completed(handle: HANDLE, overlapped: &mut OVERLAPPED) -> Result<u32, String> {
    const ERROR_IO_INCOMPLETE: u32 = 996;
    loop {
        let mut bytes: u32 = 0;
        let result = unsafe { GetOverlappedResult(handle, overlapped, &mut bytes, false) };
        match result {
            Ok(()) => return Ok(bytes / std::mem::size_of::<SHORT>() as u32),
            Err(e) => {
                let hr = e.code().0;
                let code = (hr as u32) & 0xFFFF;
                if code != ERROR_IO_INCOMPLETE { return Err(format!("GetOverlappedResult() failed: {e}")); }
                unsafe { Sleep(1) };
            }
        }
    }
}

fn fill_interleaved(buf: *mut SHORT, words: usize, rate_hz: f64, params: &GeneratorParams, phase0: &mut f64, phase1: &mut f64, pairs_generated: &mut u64, total_pairs: u64, device: &Lusbapi) {
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
            (make_voltage_sample(v0, 0, device), make_voltage_sample(v1, 1, device))
        } else {
            (make_voltage_sample(params.offset_volts, 0, device), make_voltage_sample(params.offset_volts, 1, device))
        };
        unsafe { *buf.add(2 * i) = s0; *buf.add(2 * i + 1) = s1; }
    }
}

fn normalise_phase(mut p: f64) -> f64 {
    let two_pi = 2.0 * PI;
    p %= two_pi;
    if p < 0.0 { p += two_pi; }
    p
}

fn make_voltage_sample(voltage: f64, channel: usize, device: &Lusbapi) -> SHORT {
    let normalized = (voltage / DAC_FULL_SCALE_VOLTS).clamp(-1.0, 1.0);
    let raw = normalized * 32767.0;
    let a = device.module_description.dac_offset_calibration(channel);
    let b = device.module_description.dac_scale_calibration(channel);
    clamp_to_short((raw + a) * b)
}

fn clamp_to_short(x: f64) -> SHORT {
    let x = x.clamp(-32768.0, 32767.0);
    if x >= 0.0 { (x + 0.5) as SHORT } else { (x - 0.5) as SHORT }
}

fn print_run_info(params: &GeneratorParams, requested_rate: f64, actual_rate: f64, total_pairs: u64, device: &Lusbapi) {
    let desc = &device.module_description;
    println!("Module:              E14-140, serial: {}", desc.serial_number());
    println!("Requested DAC rate:  {:.3} kHz", requested_rate / 1000.0);
    println!("Actual DAC rate:     {:.3} kHz", actual_rate / 1000.0);
    println!("Duration:            {:.3} s", params.duration_sec);
    println!("Ch0 frequency:       {:.3} Hz", params.freq_ch0_hz);
    println!("Ch1 frequency:       {:.3} Hz", params.freq_ch1_hz);
    println!("Amplitude:           {:.4} V", params.amplitude_volts);
    println!("DC offset:           {:.4} V", params.offset_volts);
    println!("Total sample pairs:  {total_pairs}");
}
