//! Win32 Bluetooth-stack implementation of [`PlatformScanner`].
//!
//! Flow per scan cycle:
//! 1. `BluetoothFindFirstDevice` with `fIssueInquiry = TRUE` — does a
//!    real over-the-air inquiry plus surfaces remembered/connected ones.
//! 2. Filter by name prefix `"Nintendo RVL-CNT-01"`.
//! 3. If unauthenticated: register an auth callback, kick off
//!    `BluetoothAuthenticateDeviceEx`. The callback handles the legacy
//!    `BLUETOOTH_AUTHENTICATION_METHOD_LEGACY` request by sending the
//!    Wiimote's own MAC reversed as a 6-byte raw PIN — what the Wiimote
//!    expects when paired via the 1+2-button trick.
//! 4. If unconnected: `BluetoothSetServiceState` with the HID service
//!    GUID to wake the HID profile on the OS side. From there hidapi
//!    enumeration picks the device up and the rest of the daemon runs.

use super::ScannerEvent;
use crossbeam_channel::Sender;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, info, warn};
use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_AUTHENTICATE_RESPONSE, BLUETOOTH_AUTHENTICATION_CALLBACK_PARAMS,
    BLUETOOTH_AUTHENTICATION_METHOD_LEGACY, BLUETOOTH_DEVICE_INFO,
    BLUETOOTH_DEVICE_SEARCH_PARAMS, BluetoothAuthenticateDeviceEx, BluetoothFindDeviceClose,
    BluetoothFindFirstDevice, BluetoothFindNextDevice, BluetoothRegisterForAuthenticationEx,
    BluetoothSendAuthenticationResponseEx, BluetoothSetServiceState,
    BluetoothUnregisterAuthentication, MITMProtectionNotRequired,
};
use windows::Win32::Foundation::{BOOL, ERROR_SUCCESS, FALSE, HANDLE, HWND, TRUE};
use windows::core::GUID;

/// Bluetooth HID service class GUID (0x1124 in the BT base).
const HID_SERVICE_GUID: GUID = GUID::from_u128(0x0000_1124_0000_1000_8000_0080_5F9B_34FB);
/// `BluetoothSetServiceState` flag to enable a service.
const BLUETOOTH_SERVICE_ENABLE: u32 = 0x01;

const WIIMOTE_NAME_PREFIX: &str = "Nintendo RVL-CNT-01";

pub struct PlatformScanner {
    events: Sender<ScannerEvent>,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformScanner {
    pub fn new(events: Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Ok(Self {
            events,
            quit: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }
        let events = self.events.clone();
        let quit = self.quit.clone();
        let h = thread::Builder::new()
            .name("bt-scan".into())
            .spawn(move || scan_loop(events, quit))?;
        self.thread = Some(h);
        Ok(())
    }
}

impl Drop for PlatformScanner {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------
// Scan loop
// ---------------------------------------------------------------------

fn scan_loop(events: Sender<ScannerEvent>, quit: Arc<AtomicBool>) {
    info!("bluetooth scan loop started");
    while !quit.load(Ordering::Relaxed) {
        let devices = match inquiry() {
            Ok(v) => v,
            Err(e) => {
                let _ = events.send(ScannerEvent::Error(format!("inquiry: {e}")));
                Vec::new()
            }
        };

        for dev in devices {
            if !is_wiimote_name(&dev.name) {
                continue;
            }
            let addr = bt_addr_u64(&dev.info);
            let paired = dev.info.fAuthenticated.as_bool();
            let connected = dev.info.fConnected.as_bool();
            let _ = events.send(ScannerEvent::Discovered {
                addr,
                name: dev.name.clone(),
                paired,
                connected,
            });

            if !paired {
                let _ = events.send(ScannerEvent::Pairing { addr });
                match pair(&dev.info) {
                    Ok(()) => {
                        let _ = events.send(ScannerEvent::Paired { addr });
                    }
                    Err(e) => {
                        let _ = events.send(ScannerEvent::PairFailed { addr, reason: e });
                        continue;
                    }
                }
            }

            if !connected {
                match enable_hid_service(&dev.info) {
                    Ok(()) => {
                        let _ = events.send(ScannerEvent::HidEnabled { addr });
                    }
                    Err(e) => {
                        warn!("enable HID on {:012x}: {e}", addr);
                        let _ = events.send(ScannerEvent::Error(format!("enable HID: {e}")));
                    }
                }
            }
        }

        // Sleep ~5 s in 200 ms slices so quit is responsive.
        for _ in 0..25 {
            if quit.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    debug!("bluetooth scan loop stopped");
}

// ---------------------------------------------------------------------
// Inquiry
// ---------------------------------------------------------------------

struct DiscoveredDevice {
    info: BLUETOOTH_DEVICE_INFO,
    name: String,
}

fn inquiry() -> Result<Vec<DiscoveredDevice>, String> {
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: TRUE,
        fReturnRemembered: TRUE,
        fReturnUnknown: TRUE,
        fReturnConnected: TRUE,
        fIssueInquiry: TRUE,
        cTimeoutMultiplier: 4, // ~5 sec
        hRadio: HANDLE::default(),
    };

    let mut info: BLUETOOTH_DEVICE_INFO = unsafe { zeroed() };
    info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

    let h = match unsafe { BluetoothFindFirstDevice(&params, &mut info) } {
        Ok(h) => h,
        Err(_) => return Ok(Vec::new()), // typically ERROR_NO_MORE_ITEMS
    };

    let mut out = Vec::new();
    loop {
        out.push(DiscoveredDevice {
            info,
            name: u16_array_to_string(&info.szName),
        });

        info = unsafe { zeroed() };
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

        if unsafe { BluetoothFindNextDevice(h, &mut info) }.is_err() {
            break;
        }
    }

    let _ = unsafe { BluetoothFindDeviceClose(h) };
    Ok(out)
}

fn is_wiimote_name(s: &str) -> bool {
    s.starts_with(WIIMOTE_NAME_PREFIX)
}

fn bt_addr_u64(info: &BLUETOOTH_DEVICE_INFO) -> u64 {
    unsafe { info.Address.Anonymous.ullLong }
}

fn u16_array_to_string(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

// ---------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------

/// Lives on the heap behind a raw pointer for the duration of a single
/// `BluetoothAuthenticateDeviceEx` call; the callback dereferences it
/// to read the PIN.
struct PairContext {
    pin: [u8; 6],
}

unsafe extern "system" fn auth_callback(
    pv_param: *const c_void,
    p_params: *const BLUETOOTH_AUTHENTICATION_CALLBACK_PARAMS,
) -> BOOL {
    let params = &*p_params;
    if params.authenticationMethod != BLUETOOTH_AUTHENTICATION_METHOD_LEGACY {
        warn!(
            "wiimote auth: unexpected method {:?}",
            params.authenticationMethod
        );
        return FALSE;
    }
    let ctx = &*(pv_param as *const PairContext);

    let mut response: BLUETOOTH_AUTHENTICATE_RESPONSE = zeroed();
    response.bthAddressRemote = params.deviceInfo.Address;
    response.authMethod = BLUETOOTH_AUTHENTICATION_METHOD_LEGACY;
    response.Anonymous.pinInfo.pin[..6].copy_from_slice(&ctx.pin);
    response.Anonymous.pinInfo.pinLength = 6;
    response.negativeResponse = 0;

    let err = BluetoothSendAuthenticationResponseEx(HANDLE::default(), &response);
    if err == ERROR_SUCCESS.0 {
        debug!("wiimote auth: PIN response sent");
        TRUE
    } else {
        warn!("wiimote auth: SendAuthenticationResponseEx 0x{:08x}", err);
        FALSE
    }
}

fn pair(info: &BLUETOOTH_DEVICE_INFO) -> Result<(), String> {
    let bytes = unsafe { info.Address.Anonymous.rgBytes };
    // Wiimote in 1+2 mode expects PIN = its own MAC, reversed.
    let pin = [bytes[5], bytes[4], bytes[3], bytes[2], bytes[1], bytes[0]];
    let ctx = Box::into_raw(Box::new(PairContext { pin }));

    let mut info_local = *info;
    let mut h_callback: isize = 0;
    let err = unsafe {
        BluetoothRegisterForAuthenticationEx(
            Some(&info_local),
            &mut h_callback,
            Some(auth_callback),
            Some(ctx as *const c_void),
        )
    };
    if err != ERROR_SUCCESS.0 {
        let _ = unsafe { Box::from_raw(ctx) };
        return Err(format!("RegisterForAuthenticationEx 0x{err:08x}"));
    }

    let auth_err = unsafe {
        BluetoothAuthenticateDeviceEx(
            HWND::default(),
            HANDLE::default(),
            &mut info_local,
            None,
            MITMProtectionNotRequired,
        )
    };

    let _ = unsafe { BluetoothUnregisterAuthentication(h_callback) };
    let _ = unsafe { Box::from_raw(ctx) };

    if auth_err != ERROR_SUCCESS.0 {
        return Err(format!("AuthenticateDeviceEx 0x{auth_err:08x}"));
    }
    Ok(())
}

fn enable_hid_service(info: &BLUETOOTH_DEVICE_INFO) -> Result<(), String> {
    let info_local = *info;
    let err = unsafe {
        BluetoothSetServiceState(
            HANDLE::default(),
            &info_local,
            &HID_SERVICE_GUID,
            BLUETOOTH_SERVICE_ENABLE,
        )
    };
    if err != ERROR_SUCCESS.0 {
        return Err(format!("SetServiceState 0x{err:08x}"));
    }
    Ok(())
}
