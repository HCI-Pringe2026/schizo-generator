//! Safe Rust bindings for `Lusbapi.dll` (LCard E14-140 DAC).
//!
//! The original C++ project uses a COM-like vtable (`ILE140`) obtained from
//! `CreateLInstance("e140")`.  Because Rust cannot link against this DLL at
//! compile time (no import lib), we load it **dynamically** with
//! `LoadLibraryW` / `GetProcAddress` and call function pointers directly.
//!
//! The vtable layout below was reverse-engineered from the public LCard
//! header `Lusbapi.h` and the C++ source.

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::CString;
use std::os::raw::{c_char, c_uchar, c_ushort, c_void};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{BOOL, HANDLE, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

// ── primitive aliases matching Windows headers ───────────────────────────────

pub type WORD  = c_ushort;
pub type DWORD = u32;
pub type SHORT = i16;
pub type BYTE  = c_uchar;

// ── Lusbapi version constant (from Lusbapi.h) ────────────────────────────────

pub const CURRENT_VERSION_LUSBAPI: DWORD = 0x30002;
pub const MAX_VIRTUAL_SLOTS_QUANTITY_LUSBAPI: WORD = 127;

const LUSBAPI_DLL: &str = "Lusbapi.dll";
const IMAGE_FILE_MACHINE_I386: u16 = 0x014c;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
const NAME_LINE_LENGTH_LUSBAPI: usize = 25;
const COMMENT_LINE_LENGTH_LUSBAPI: usize = 256;
const ADC_CALIBR_COEFS_QUANTITY_LUSBAPI: usize = 128;
const DAC_CALIBR_COEFS_QUANTITY_LUSBAPI: usize = 128;

// ── Lusbapi structs (must match the ABI exactly) ──────────────────────────────

/// Overlapped I/O request passed to `WriteData`.
#[repr(C)]
pub struct IO_REQUEST_LUSBAPI {
    pub Buffer: *mut SHORT,
    pub NumberOfWordsToPass: DWORD,
    pub NumberOfWordsPassed: DWORD,
    pub Overlapped: *mut windows::Win32::System::IO::OVERLAPPED,
    pub TimeOut: DWORD,
}

unsafe impl Send for IO_REQUEST_LUSBAPI {}

#[repr(C, packed)]
pub struct LAST_ERROR_INFO_LUSBAPI {
    pub ErrorString: [BYTE; 256],
    pub ErrorNumber: DWORD,
}

#[repr(C, packed)]
pub struct VERSION_INFO_LUSBAPI {
    pub Version: [BYTE; 10],
    pub Date: [BYTE; 14],
    pub Manufacturer: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub Author: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

/// Module identity sub-struct inside `MODULE_DESCRIPTION_E140`.
#[repr(C, packed)]
pub struct MODULE_DESCRIPTION_MODULE_E140 {
    pub CompanyName: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub DeviceName: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub SerialNumber: [BYTE; 16],
    pub Revision: BYTE,
    pub Modification: BYTE,
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

#[repr(C, packed)]
pub struct INTERFACE_INFO_LUSBAPI {
    pub Active: BOOL,
    pub Name: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

#[repr(C, packed)]
pub struct MCU_INFO_LUSBAPI {
    pub Active: BOOL,
    pub Name: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub ClockRate: f64,
    pub Version: VERSION_INFO_LUSBAPI,
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

#[repr(C, packed)]
pub struct ADC_INFO_LUSBAPI {
    pub Active: BOOL,
    pub Name: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub OffsetCalibration: [f64; ADC_CALIBR_COEFS_QUANTITY_LUSBAPI],
    pub ScaleCalibration: [f64; ADC_CALIBR_COEFS_QUANTITY_LUSBAPI],
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

#[repr(C, packed)]
pub struct DAC_INFO_LUSBAPI {
    pub Active: BOOL,
    pub Name: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub OffsetCalibration: [f64; DAC_CALIBR_COEFS_QUANTITY_LUSBAPI],
    pub ScaleCalibration: [f64; DAC_CALIBR_COEFS_QUANTITY_LUSBAPI],
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

#[repr(C, packed)]
pub struct DIGITAL_IO_INFO_LUSBAPI {
    pub Active: BOOL,
    pub Name: [BYTE; NAME_LINE_LENGTH_LUSBAPI],
    pub InLinesQuantity: WORD,
    pub OutLinesQuantity: WORD,
    pub Comment: [BYTE; COMMENT_LINE_LENGTH_LUSBAPI],
}

/// Full module description (from `GET_MODULE_DESCRIPTION`).
#[repr(C, packed)]
pub struct MODULE_DESCRIPTION_E140 {
    pub Module: MODULE_DESCRIPTION_MODULE_E140,
    pub Interface: INTERFACE_INFO_LUSBAPI,
    pub Mcu: MCU_INFO_LUSBAPI,
    pub Adc: ADC_INFO_LUSBAPI,
    pub Dac: DAC_INFO_LUSBAPI,
    pub DigitalIo: DIGITAL_IO_INFO_LUSBAPI,
}

impl MODULE_DESCRIPTION_E140 {
    pub fn serial_number(&self) -> String {
        c_string_from_bytes(unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!(self.Module.SerialNumber).cast::<u8>(),
                16,
            )
        })
    }

    pub fn revision(&self) -> BYTE {
        self.Module.Revision
    }

    pub fn dac_is_accessible(&self) -> bool {
        self.Dac.Active.as_bool()
    }

    pub fn dac_offset_calibration(&self, channel: usize) -> f64 {
        read_unaligned_f64_from_array(
            std::ptr::addr_of!(self.Dac.OffsetCalibration).cast::<f64>(),
            channel,
        )
    }

    pub fn dac_scale_calibration(&self, channel: usize) -> f64 {
        read_unaligned_f64_from_array(
            std::ptr::addr_of!(self.Dac.ScaleCalibration).cast::<f64>(),
            channel,
        )
    }
}

/// DAC streaming parameters (from `SET_DAC_PARS` / `GET_DAC_PARS`).
#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct DAC_PARS_E140 {
    pub SyncWithADC: BYTE,
    pub SetZeroOnStop: BYTE,
    pub DacRate: f64, // kHz
}

// ── COM-style vtable (ILE140) ─────────────────────────────────────────────────
//
// The DLL exports a single factory `CreateLInstance(name: *const c_char) -> *mut c_void`.
// The returned pointer is a pointer to an object whose first field is a vtable pointer.
// The vtable methods we use are listed below in declaration order matching Lusbapi.h.

type FnCreateLInstance  = unsafe extern "system" fn(name: *const c_char) -> *mut c_void;
type FnGetDllVersion    = unsafe extern "system" fn() -> DWORD;

// Methods accessed via vtable pointer on the ILE140 object.
// Each entry is WINAPI, which maps to `extern "system"` on Rust targets.
type VtFnOpenLDevice    = unsafe extern "system" fn(*mut c_void, slot: WORD) -> BOOL;
type VtFnNoArgsBool     = unsafe extern "system" fn(*mut c_void) -> BOOL;
type VtFnGetModuleName  = unsafe extern "system" fn(*mut c_void, name: *mut c_char) -> BOOL;
type VtFnGetModuleHandle= unsafe extern "system" fn(*mut c_void) -> HANDLE;
type VtFnGetUsbSpeed    = unsafe extern "system" fn(*mut c_void, speed: *mut BYTE) -> BOOL;
type VtFnGetLastErrorInfo =
    unsafe extern "system" fn(*mut c_void, info: *mut LAST_ERROR_INFO_LUSBAPI) -> BOOL;
type VtFnGetModDesc     =
    unsafe extern "system" fn(*mut c_void, desc: *mut MODULE_DESCRIPTION_E140) -> BOOL;
type VtFnGetDacPars     = unsafe extern "system" fn(*mut c_void, pars: *mut DAC_PARS_E140) -> BOOL;
type VtFnSetDacPars     = unsafe extern "system" fn(*mut c_void, pars: *mut DAC_PARS_E140) -> BOOL;
type VtFnWriteData      =
    unsafe extern "system" fn(*mut c_void, req: *mut IO_REQUEST_LUSBAPI) -> BOOL;
type VtFnDacSample      =
    unsafe extern "system" fn(*mut c_void, sample: *mut SHORT, channel: WORD) -> BOOL;
type VtFnBoolPtr        = unsafe extern "system" fn(*mut c_void, ptr: *mut c_void) -> BOOL;
type VtFnBoolWord       = unsafe extern "system" fn(*mut c_void, value: WORD) -> BOOL;

/// Vtable layout for `ILE140` — declaration order from Lusbapi.h.
#[repr(C)]
struct ILE140Vtbl {
    OpenLDevice: VtFnOpenLDevice,
    CloseLDevice: VtFnNoArgsBool,
    ReleaseLInstance: VtFnNoArgsBool,
    GetModuleHandle: VtFnGetModuleHandle,
    GetModuleName: VtFnGetModuleName,
    GetUsbSpeed: VtFnGetUsbSpeed,
    LowPowerMode: VtFnBoolWord,
    GetLastErrorInfo: VtFnGetLastErrorInfo,

    GET_ADC_PARS: *const c_void,
    SET_ADC_PARS: *const c_void,
    START_ADC: *const c_void,
    STOP_ADC: *const c_void,
    ADC_KADR: *const c_void,
    ADC_SAMPLE: *const c_void,
    ReadData: *const c_void,
    GET_DAC_PARS: VtFnGetDacPars,
    SET_DAC_PARS: VtFnSetDacPars,
    START_DAC: VtFnNoArgsBool,
    STOP_DAC: VtFnNoArgsBool,
    WriteData: VtFnWriteData,
    DAC_SAMPLE: VtFnDacSample,
    DAC_SAMPLES: *const c_void,
    ENABLE_TTL_OUT: *const c_void,
    TTL_IN: *const c_void,
    TTL_OUT: *const c_void,
    ENABLE_FLASH_WRITE: *const c_void,
    READ_FLASH_ARRAY: VtFnBoolPtr,
    WRITE_FLASH_ARRAY: VtFnBoolPtr,
    GET_MODULE_DESCRIPTION: VtFnGetModDesc,
    SAVE_MODULE_DESCRIPTION: VtFnGetModDesc,
    GetArray: *const c_void,
    PutArray: *const c_void,
}

#[repr(C)]
struct ILE140Object {
    vtbl: *const ILE140Vtbl,
}

fn c_string_from_bytes(bytes: &[u8]) -> String {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

fn read_unaligned_f64_from_array(base: *const f64, index: usize) -> f64 {
    unsafe { base.add(index).read_unaligned() }
}

fn last_error_suffix(obj: *mut ILE140Object) -> String {
    if obj.is_null() {
        return String::new();
    }

    let mut info = LAST_ERROR_INFO_LUSBAPI {
        ErrorString: [0; 256],
        ErrorNumber: 0,
    };
    let ok = unsafe { ((*(*obj).vtbl).GetLastErrorInfo)(obj as *mut c_void, &mut info) };
    if !ok.as_bool() {
        return String::new();
    }

    let message = c_string_from_bytes(&info.ErrorString);
    let number = info.ErrorNumber;
    if message.is_empty() {
        format!(" (Lusbapi error {number})")
    } else {
        format!(": {message} (Lusbapi error {number})")
    }
}

fn locate_dll(name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join(name));
        }
    }

    candidates.push(PathBuf::from(name));

    if let Some(path_var) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path_var).map(|dir| dir.join(name)));
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn validate_dll_architecture(path: &Path) -> Result<(), String> {
    let machine = read_pe_machine(path)?;
    if expected_machine_matches(machine) {
        return Ok(());
    }

    Err(format!(
        "{} is {}, but this executable is {}. Use a DLL with the same bitness as \
         the executable. For the bundled 32-bit Lusbapi.dll, build with: \
         rustup target add i686-pc-windows-msvc; cargo build --release --target \
         i686-pc-windows-msvc. For a 64-bit executable, copy the 64-bit vendor \
         Lusbapi.dll next to the .exe.",
        path.display(),
        machine_arch_name(machine),
        process_arch_name(),
    ))
}

fn read_pe_machine(path: &Path) -> Result<u16, String> {
    let data = std::fs::read(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    if data.len() < 0x40 || &data[0..2] != b"MZ" {
        return Err(format!("{} is not a Windows PE DLL", path.display()));
    }

    let pe_offset =
        u32::from_le_bytes([data[0x3c], data[0x3d], data[0x3e], data[0x3f]]) as usize;
    if data.len() < pe_offset + 6 || &data[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return Err(format!("{} is not a valid Windows PE DLL", path.display()));
    }

    Ok(u16::from_le_bytes([data[pe_offset + 4], data[pe_offset + 5]]))
}

fn expected_machine_matches(machine: u16) -> bool {
    match machine {
        #[cfg(target_arch = "x86")]
        IMAGE_FILE_MACHINE_I386 => true,
        #[cfg(target_arch = "x86_64")]
        IMAGE_FILE_MACHINE_AMD64 => true,
        #[cfg(target_arch = "aarch64")]
        IMAGE_FILE_MACHINE_ARM64 => true,
        _ => false,
    }
}

fn machine_arch_name(machine: u16) -> &'static str {
    match machine {
        IMAGE_FILE_MACHINE_I386 => "32-bit (x86)",
        IMAGE_FILE_MACHINE_AMD64 => "64-bit (x64)",
        IMAGE_FILE_MACHINE_ARM64 => "64-bit (ARM64)",
        _ => "an unsupported Windows architecture",
    }
}

fn process_arch_name() -> &'static str {
    #[cfg(target_arch = "x86")]
    {
        "32-bit (x86)"
    }
    #[cfg(target_arch = "x86_64")]
    {
        "64-bit (x64)"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "64-bit (ARM64)"
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "this process architecture"
    }
}

// ── Public safe wrapper ───────────────────────────────────────────────────────

/// Loaded Lusbapi.dll handle + opened ILE140 instance.
pub struct Lusbapi {
    _lib: HMODULE, // keep DLL loaded
    obj: *mut ILE140Object,
    pub module_handle: HANDLE,
    pub module_description: MODULE_DESCRIPTION_E140,
    pub dac_pars: DAC_PARS_E140,
    pub usb_speed: BYTE,
}

// SAFETY: We never share across threads while the device is open.
unsafe impl Send for Lusbapi {}

impl Lusbapi {
    /// Load the DLL and open the first E14-140 found.
    pub fn open() -> Result<Self, String> {
        // ---------- load DLL ----------
        let dll_path_wide: Vec<u16> = format!("{LUSBAPI_DLL}\0").encode_utf16().collect();
        let lib = unsafe {
            LoadLibraryW(windows::core::PCWSTR(dll_path_wide.as_ptr()))
                .map_err(|e| {
                    let arch_hint = locate_dll(LUSBAPI_DLL)
                        .and_then(|path| validate_dll_architecture(&path).err())
                        .map(|msg| format!(" {msg}"))
                        .unwrap_or_default();
                    format!("LoadLibraryW({LUSBAPI_DLL}) failed: {e}.{arch_hint}")
                })?
        };

        // ---------- version check ----------
        let get_dll_version: FnGetDllVersion = unsafe {
            let s = CString::new("GetDllVersion").unwrap();
            let addr = GetProcAddress(lib, windows::core::PCSTR(s.as_ptr() as *const u8))
                .ok_or("GetProcAddress(GetDllVersion) failed")?;
            std::mem::transmute(addr)
        };
        let ver = unsafe { get_dll_version() };
        if (ver >> 16) != (CURRENT_VERSION_LUSBAPI >> 16) {
            return Err(format!(
                "Unsupported Lusbapi.dll major version: got 0x{ver:x}, expected 0x{CURRENT_VERSION_LUSBAPI:x}"
            ));
        } else if ver != CURRENT_VERSION_LUSBAPI {
            eprintln!(
                "Warning: Lusbapi.dll version is 0x{ver:x}; expected 0x{CURRENT_VERSION_LUSBAPI:x}. Continuing because the major version matches."
            );
        }

        // ---------- create instance ----------
        let create_instance: FnCreateLInstance = unsafe {
            let s = CString::new("CreateLInstance").unwrap();
            let addr = GetProcAddress(lib, windows::core::PCSTR(s.as_ptr() as *const u8))
                .ok_or("GetProcAddress(CreateLInstance) failed")?;
            std::mem::transmute(addr)
        };
        let obj_ptr = unsafe {
            let name = CString::new("e140").unwrap();
            create_instance(name.as_ptr())
        };
        if obj_ptr.is_null() {
            return Err("CreateLInstance(\"e140\") returned null".into());
        }
        let obj = obj_ptr as *mut ILE140Object;

        // ---------- open first available slot ----------
        let mut opened_slot = MAX_VIRTUAL_SLOTS_QUANTITY_LUSBAPI;
        for slot in 0..MAX_VIRTUAL_SLOTS_QUANTITY_LUSBAPI {
            let ok = unsafe { ((*(*obj).vtbl).OpenLDevice)(obj as *mut c_void, slot) };
            if ok.as_bool() {
                opened_slot = slot;
                break;
            }
        }
        if opened_slot == MAX_VIRTUAL_SLOTS_QUANTITY_LUSBAPI {
            return Err(format!(
                "E14-140 not found in any virtual slot{}",
                last_error_suffix(obj)
            ));
        }

        // ---------- get module handle ----------
        let module_handle =
            unsafe { ((*(*obj).vtbl).GetModuleHandle)(obj as *mut c_void) };
        if module_handle.is_invalid() {
            return Err(format!(
                "GetModuleHandle() returned INVALID_HANDLE_VALUE{}",
                last_error_suffix(obj)
            ));
        }

        // ---------- verify module name ----------
        let mut name_buf = [0i8; 8];
        let ok = unsafe { ((*(*obj).vtbl).GetModuleName)(obj as *mut c_void, name_buf.as_mut_ptr()) };
        if !ok.as_bool() {
            return Err(format!("GetModuleName() failed{}", last_error_suffix(obj)));
        }
        let name_str = unsafe { std::ffi::CStr::from_ptr(name_buf.as_ptr()) }
            .to_str()
            .unwrap_or("");
        if name_str != "E140" {
            return Err(format!("Connected module is '{name_str}', expected 'E140'"));
        }

        // ---------- USB speed ----------
        let mut usb_speed: BYTE = 0;
        let ok = unsafe { ((*(*obj).vtbl).GetUsbSpeed)(obj as *mut c_void, &mut usb_speed) };
        if !ok.as_bool() {
            return Err(format!("GetUsbSpeed() failed{}", last_error_suffix(obj)));
        }

        // ---------- module description ----------
        // Zero-init the struct; the sizes of the ADC padding area must be correct
        // for the offsets to the DAC fields to match the DLL's expectations.
        let mut module_description: MODULE_DESCRIPTION_E140 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            ((*(*obj).vtbl).GET_MODULE_DESCRIPTION)(obj as *mut c_void, &mut module_description)
        };
        if !ok.as_bool() {
            return Err(format!(
                "GET_MODULE_DESCRIPTION() failed{}",
                last_error_suffix(obj)
            ));
        }
        if !module_description.dac_is_accessible() {
            return Err("DAC not accessible on this E14-140".into());
        }
        let revision = module_description.revision();
        if revision < b'B' {
            return Err(format!(
                "Streaming DAC requires E14-140 Rev. B or higher; got Rev. {}",
                revision as char
            ));
        }

        // ---------- DAC params ----------
        let mut dac_pars = DAC_PARS_E140::default();
        let ok = unsafe { ((*(*obj).vtbl).GET_DAC_PARS)(obj as *mut c_void, &mut dac_pars) };
        if !ok.as_bool() {
            return Err(format!("GET_DAC_PARS() failed{}", last_error_suffix(obj)));
        }

        Ok(Self {
            _lib: lib,
            obj,
            module_handle,
            module_description,
            dac_pars,
            usb_speed,
        })
    }

    // ── Thin safe wrappers around vtable calls ────────────────────────────────

    pub fn set_dac_pars(&mut self, pars: &mut DAC_PARS_E140) -> bool {
        unsafe { ((*(*self.obj).vtbl).SET_DAC_PARS)(self.obj as *mut c_void, pars) }.as_bool()
    }

    pub fn start_dac(&self) -> bool {
        unsafe { ((*(*self.obj).vtbl).START_DAC)(self.obj as *mut c_void) }.as_bool()
    }

    pub fn stop_dac(&self) -> bool {
        unsafe { ((*(*self.obj).vtbl).STOP_DAC)(self.obj as *mut c_void) }.as_bool()
    }

    pub fn write_data(&self, req: &mut IO_REQUEST_LUSBAPI) -> bool {
        unsafe { ((*(*self.obj).vtbl).WriteData)(self.obj as *mut c_void, req) }.as_bool()
    }

    pub fn dac_sample(&self, sample: &mut SHORT, channel: WORD) -> bool {
        unsafe { ((*(*self.obj).vtbl).DAC_SAMPLE)(self.obj as *mut c_void, sample, channel) }
            .as_bool()
    }

    /// Release the COM-like instance back to the DLL.
    fn release(&mut self) {
        if !self.obj.is_null() {
            unsafe {
                let _ = ((*(*self.obj).vtbl).ReleaseLInstance)(self.obj as *mut c_void);
            }
            self.obj = std::ptr::null_mut();
        }
    }
}

impl Drop for Lusbapi {
    fn drop(&mut self) {
        // Best-effort zero the DAC outputs and stop.
        self.stop_dac();
        let mut zero: SHORT = 0;
        self.dac_sample(&mut zero, 0);
        self.dac_sample(&mut zero, 1);
        self.release();
    }
}
