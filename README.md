# lcard_dac_signal_generator

Rust rewrite of the [LCard E14-140 DAC signal generator].

## What it does

Streams a continuous sine wave (one per DAC channel) to an **LCard E14-140**
USB data acquisition module via the vendor `Lusbapi.dll`.  Both channels are
driven independently with configurable frequency, phase, amplitude, and DC
offset.

## Differences from the original C++/Qt version

| Aspect | C++ original | This Rust rewrite |
|---|---|---|
| GUI toolkit | Qt 5/6 (QMainWindow, signals/slots) | `crossterm` terminal TUI (no native deps) |
| Threading | `QThread` | `std::thread` + `Arc<AtomicBool>` stop flag |
| DLL loading | Static link via `.lib` import library | Dynamic `LoadLibraryW` / vtable at runtime |
| Error handling | C++ exceptions | `Result<_, String>` throughout |
| CLI mode | No | Yes — pass args directly |

## Requirements

* Windows — the `Lusbapi.dll` is Windows-only.
* An LCard E14-140 module connected via USB.
* `Lusbapi.dll` in the same directory as the executable (or on `PATH`).
* The executable and `Lusbapi.dll` must have the same bitness. The DLL bundled
  in this repository is 32-bit (`PE32 Intel 80386`), so it requires a 32-bit
  build of the program unless you replace it with a 64-bit vendor DLL.
* Rust toolchain ≥ 1.70 (edition 2021).

## Building

### With the bundled 32-bit `Lusbapi.dll`

```powershell
rustup target add i686-pc-windows-msvc
cargo build --release --target i686-pc-windows-msvc
# → target\i686-pc-windows-msvc\release\lcard_dac_signal_generator.exe
```

Copy `Lusbapi.dll` next to that `.exe` before running.

### Cross-building the 32-bit executable from macOS/Linux

```sh
cargo install cargo-xwin
rustup target add i686-pc-windows-msvc
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo xwin build --release --target i686-pc-windows-msvc --cross-compiler clang
```

### With a 64-bit vendor `Lusbapi.dll`

```powershell
cargo build --release
# → target\release\lcard_dac_signal_generator.exe
```

Copy the 64-bit `Lusbapi.dll` next to the `.exe` before running.

## Troubleshooting

If startup fails with `LoadLibraryW(Lusbapi.dll) failed: ... is not a valid
Win32 application`, the DLL bitness does not match the executable bitness.
Use the 32-bit build command above for the bundled DLL, or replace the DLL with
the 64-bit version from LCard when running a 64-bit build.

## Usage

### Interactive TUI (default)

```
lcard_dac_signal_generator
```

Keys:

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | Move between fields |
| `↑` / `↓` | Increase / decrease value |
| `+` / `-` | Coarse step (×10) |
| `Enter` | Start generation |
| `Esc` | Abort running generation |
| `Q` | Quit |

### Headless CLI

```
lcard_dac_signal_generator <duration_s> <freq0_hz> <freq1_hz> \
                            [phase0_deg] [phase1_deg] \
                            [amplitude_v] [offset_v]
```

Example — 10 s, 50 Hz on Ch0, 100 Hz on Ch1, 90° phase offset, 1.5 V amplitude:

```
lcard_dac_signal_generator 10 50 100 0 90 1.5 0
```

## Architecture

```
src/
  main.rs            Entry point: CLI arg parsing, Ctrl-C hook, mode dispatch
  ui.rs              crossterm TUI (replaces Qt MainWindow + GeneratorThread)
  signal_generator.rs Core DAC streaming logic (mirrors SignalGenerator.cpp)
  lusbapi.rs         FFI bindings: LoadLibrary + vtable wrappers for Lusbapi.dll
```

### Key design decisions

**Dynamic DLL loading** — `Lusbapi.dll` has no public Rust import crate and
ships no stable `.lib`.  We call `LoadLibraryW` at startup, then cast the
vtable pointer from `CreateLInstance("e140")` to our own `#[repr(C)]`
vtable struct.  The vtable slot offsets were verified against `Lusbapi.h`.

**Double-buffered async I/O** — Mirrors the C++ original exactly: two
`IO_REQUEST_LUSBAPI` structs backed by Windows `OVERLAPPED` events.  One
buffer plays while the other is being refilled, eliminating output gaps.

**Stop flag** — An `Arc<AtomicBool>` is shared between the TUI event loop
and the generation thread so the user can abort at any time without
`unsafe` or OS signal tricks in the hot path.
