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

use windows::Win32::Foundation::{BOOL, HANDLE, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

// ── primitive aliases matching Windows headers ───────────────────────────────

pub type WORD  = c_ushort;
pub type DWORD = u32;
pub type SHORT = i16;
pub type BYTE  = c_uchar;

// ── Lusbapi version constant (from Lusbapi.h) ────────────────────────────────

pub const CURRENT_VERSION_LUSBAPI: DWORD = 0x30002;
pub const MAX_VIRTUAL_SLOTS_QUANTITY_LUSBAPI: WORD = 16;
pub const DAC_ACCESSIBLED_E140: BYTE = 1;

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

/// DAC calibration / capability sub-struct inside `MODULE_DESCRIPTION_E140`.
#[repr(C)]
pub struct DAC_DESCRIPTION_E140 {
    pub Active: BYTE,
    pub Channels: BYTE,
    pub Bits: BYTE,
    pub Reserved: [BYTE; 5],
    pub OffsetCalibration: [f64; 2],
    pub ScaleCalibration: [f64; 2],
}

/// Module identity sub-struct inside `MODULE_DESCRIPTION_E140`.
#[repr(C)]
pub struct MODULE_DESCRIPTION_MODULE_E140 {
    pub Revision: c_char,
    pub SerialNumber: [c_char; 9],
    pub Reserved: [BYTE; 22],
}

/// Full module description (from `GET_MODULE_DESCRIPTION`).
#[repr(C)]
pub struct MODULE_DESCRIPTION_E140 {
    pub Module: MODULE_DESCRIPTION_MODULE_E140,
    pub Adc:    [BYTE; 64], // we don't need ADC fields; pad to correct size
    pub Dac:    DAC_DESCRIPTION_E140,
}

/// DAC streaming parameters (from `SET_DAC_PARS` / `GET_DAC_PARS`).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct DAC_PARS_E140 {
    pub DacRate: f64,         // kHz
    pub SyncWithADC: WORD,
    pub SetZeroOnStop: WORD,
    pub Reserved: [BYTE; 12],
}

// ── COM-style vtable (ILE140) ─────────────────────────────────────────────────
//
// The DLL exports a single factory `CreateLInstance(name: *const c_char) -> *mut c_void`.
// The returned pointer is a pointer to an object whose first field is a vtable pointer.
// The vtable methods we use are listed below in declaration order matching Lusbapi.h.

type FnCreateLInstance  = unsafe extern "C" fn(name: *const c_char) -> *mut c_void;
type FnGetDllVersion    = unsafe extern "C" fn() -> DWORD;

// Methods accessed via vtable pointer on the ILE140 object.
// Each entry is `unsafe extern "C" fn(self_ptr: *mut c_void, ...) -> ...`
type VtFnOpenLDevice    = unsafe extern "C" fn(*mut c_void, slot: WORD) -> BOOL;
type VtFnGetModuleName  = unsafe extern "C" fn(*mut c_void, name: *mut c_char) -> BOOL;
type VtFnGetModuleHandle= unsafe extern "C" fn(*mut c_void) -> HANDLE;
type VtFnGetUsbSpeed    = unsafe extern "C" fn(*mut c_void, speed: *mut BYTE) -> BOOL;
type VtFnGetModDesc     = unsafe extern "C" fn(*mut c_void, desc: *mut MODULE_DESCRIPTION_E140) -> BOOL;
type VtFnGetDacPars     = unsafe extern "C" fn(*mut c_void, pars: *mut DAC_PARS_E140) -> BOOL;
type VtFnSetDacPars     = unsafe extern "C" fn(*mut c_void, pars: *mut DAC_PARS_E140) -> BOOL;
type VtFnStartDac       = unsafe extern "C" fn(*mut c_void) -> BOOL;
type VtFnStopDac        = unsafe extern "C" fn(*mut c_void) -> BOOL;
type VtFnWriteData      = unsafe extern "C" fn(*mut c_void, req: *mut IO_REQUEST_LUSBAPI) -> BOOL;
type VtFnDacSample      = unsafe extern "C" fn(*mut c_void, sample: *mut SHORT, channel: WORD) -> BOOL;
type VtFnRelease        = unsafe extern "C" fn(*mut c_void);

/// Vtable layout for `ILE140` — indices from Lusbapi.h (0-based).
#[repr(C)]
struct ILE140Vtbl {
    // Index 0 – destructor / QueryInterface family; skip with padding
    _reserved: [*const c_void; 3],
    // 3
    OpenLDevice:    VtFnOpenLDevice,
    // 4
    GetModuleName:  VtFnGetModuleName,
    // 5
    GetModuleHandle:VtFnGetModuleHandle,
    // 6
    GetUsbSpeed:    VtFnGetUsbSpeed,
    // 7 — padding (CloseLDevice)
    _pad7: *const c_void,
    // 8
    GET_MODULE_DESCRIPTION: VtFnGetModDesc,
    // 9 — padding (GET_ADC_PARS)
    _pad9: *const c_void,
    // 10 — padding (SET_ADC_PARS)
    _pad10: *const c_void,
    // 11
    GET_DAC_PARS: VtFnGetDacPars,
    // 12
    SET_DAC_PARS: VtFnSetDacPars,
    // 13
    START_DAC: VtFnStartDac,
    // 14
    STOP_DAC: VtFnStopDac,
    // 15
    WriteData: VtFnWriteData,
    // 16
    DAC_SAMPLE: VtFnDacSample,
    // 17
    ReleaseLInstance: VtFnRelease,
}

#[repr(C)]
struct ILE140Object {
    vtbl: *const ILE140Vtbl,
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
        let dll_path: Vec<u16> = "Lusbapi.dll\0".encode_utf16().collect();
        let lib = unsafe {
            LoadLibraryW(windows::core::PCWSTR(dll_path.as_ptr()))
                .map_err(|e| format!("LoadLibraryW(Lusbapi.dll) failed: {e}"))?
        };

        // ---------- version check ----------
        let get_dll_version: FnGetDllVersion = unsafe {
            let s = CString::new("GetDllVersion").unwrap();
            let addr = GetProcAddress(lib, windows::core::PCSTR(s.as_ptr() as *const u8))
                .ok_or("GetProcAddress(GetDllVersion) failed")?;
            std::mem::transmute(addr)
        };
        let ver = unsafe { get_dll_version() };
        if ver != CURRENT_VERSION_LUSBAPI {
            return Err(format!(
                "Lusbapi.dll version mismatch: got 0x{ver:x}, expected 0x{CURRENT_VERSION_LUSBAPI:x}"
            ));
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
            return Err("E14-140 not found in any virtual slot".into());
        }

        // ---------- get module handle ----------
        let module_handle =
            unsafe { ((*(*obj).vtbl).GetModuleHandle)(obj as *mut c_void) };
        if module_handle.is_invalid() {
            return Err("GetModuleHandle() returned INVALID_HANDLE_VALUE".into());
        }

        // ---------- verify module name ----------
        let mut name_buf = [0i8; 8];
        let ok = unsafe { ((*(*obj).vtbl).GetModuleName)(obj as *mut c_void, name_buf.as_mut_ptr()) };
        if !ok.as_bool() {
            return Err("GetModuleName() failed".into());
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
            return Err("GetUsbSpeed() failed".into());
        }

        // ---------- module description ----------
        // Zero-init the struct; the sizes of the ADC padding area must be correct
        // for the offsets to the DAC fields to match the DLL's expectations.
        let mut module_description: MODULE_DESCRIPTION_E140 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            ((*(*obj).vtbl).GET_MODULE_DESCRIPTION)(obj as *mut c_void, &mut module_description)
        };
        if !ok.as_bool() {
            return Err("GET_MODULE_DESCRIPTION() failed".into());
        }
        if module_description.Dac.Active != DAC_ACCESSIBLED_E140 {
            return Err("DAC not accessible on this E14-140".into());
        }
        if (module_description.Module.Revision as u8) < b'B' {
            return Err(format!(
                "Streaming DAC requires E14-140 Rev. B or higher; got Rev. {}",
                module_description.Module.Revision as u8 as char
            ));
        }

        // ---------- DAC params ----------
        let mut dac_pars = DAC_PARS_E140::default();
        let ok = unsafe { ((*(*obj).vtbl).GET_DAC_PARS)(obj as *mut c_void, &mut dac_pars) };
        if !ok.as_bool() {
            return Err("GET_DAC_PARS() failed".into());
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
            unsafe { ((*(*self.obj).vtbl).ReleaseLInstance)(self.obj as *mut c_void) };
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