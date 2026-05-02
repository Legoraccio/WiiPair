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
use std::collections::HashMap;
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
    BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS,
    BluetoothAuthenticateDeviceEx, BluetoothEnumerateInstalledServices,
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindFirstRadio,
    BluetoothFindNextDevice, BluetoothFindRadioClose, BluetoothGetDeviceInfo,
    BluetoothRegisterForAuthenticationEx, BluetoothRemoveDevice,
    BluetoothSendAuthenticationResponseEx, BluetoothSetServiceState,
    BluetoothUnregisterAuthentication, MITMProtectionNotRequired,
};
use windows::Win32::Foundation::{
    BOOL, CloseHandle, ERROR_INVALID_PARAMETER, ERROR_SUCCESS, FALSE, HANDLE, HWND, TRUE,
};
use windows::core::GUID;

/// Bluetooth HID service class GUID (0x1124 in the BT base).
const HID_SERVICE_GUID: GUID = GUID::from_u128(0x0000_1124_0000_1000_8000_0080_5F9B_34FB);
/// `BluetoothSetServiceState` flag to enable a service.
const BLUETOOTH_SERVICE_ENABLE: u32 = 0x01;

const WIIMOTE_NAME_PREFIX: &str = "Nintendo RVL-CNT-01";

pub struct PlatformScanner {
    events: Sender<ScannerEvent>,
    quit: Arc<AtomicBool>,
    /// When `true`, the scanner skips active BT inquiry. Bluetooth
    /// inquiry interleaves 1.28 s windows during which the radio
    /// can't service connected devices — we only want to do it when
    /// we're actually trying to find a *new* Wiimote.
    pause_inquiry: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformScanner {
    pub fn new(events: Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Ok(Self {
            events,
            quit: Arc::new(AtomicBool::new(false)),
            pause_inquiry: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }

    /// Hand the daemon the pause flag so it can suspend the BT inquiry
    /// while there's at least one Wiimote connected — otherwise the
    /// inquiry's hop windows briefly starve the active connection,
    /// causing visible "freezes" in the input stream.
    pub fn pause_handle(&self) -> Arc<AtomicBool> {
        self.pause_inquiry.clone()
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }
        let events = self.events.clone();
        let quit = self.quit.clone();
        let pause = self.pause_inquiry.clone();
        let h = thread::Builder::new()
            .name("bt-scan".into())
            .spawn(move || scan_loop(events, quit, pause))?;
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

fn scan_loop(
    events: Sender<ScannerEvent>,
    quit: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
) {
    info!("bluetooth scan loop started");
    // Per-device dedup: only re-emit Discovered when (paired, connected)
    // actually flips. Without this the UI log fills with one Discovered
    // line per inquiry cycle (every ~3 s) for every paired Wiimote.
    let mut last_seen: HashMap<u64, (bool, bool)> = HashMap::new();
    while !quit.load(Ordering::Relaxed) {
        if pause.load(Ordering::Relaxed) {
            // Tight poll while paused so the moment the daemon flips
            // the flag back (e.g. user pressed "Scan for new devices")
            // we start an inquiry within ~200 ms instead of waiting
            // out a multi-second sleep.
            for _ in 0..5 {
                if quit.load(Ordering::Relaxed) {
                    return;
                }
                if !pause.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
            continue;
        }

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
            let prev = last_seen.insert(addr, (paired, connected));
            if prev != Some((paired, connected)) {
                let _ = events.send(ScannerEvent::Discovered {
                    addr,
                    name: dev.name.clone(),
                    paired,
                    connected,
                });
            }

            if !paired {
                let _ = events.send(ScannerEvent::Pairing { addr });
                match pair(&dev.info, &events) {
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
                match enable_hid_service(&dev.info, &events) {
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

        // Cooldown between inquiries — broken into 200 ms slices so we
        // exit promptly on quit or a pause flip.
        for _ in 0..15 {
            if quit.load(Ordering::Relaxed) {
                return;
            }
            if pause.load(Ordering::Relaxed) {
                break;
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
        // 2 × 1.28 s ≈ 2.56 s of inquiry. Shorter than the previous 5 s
        // so the active Wiimote (when one is connected) is starved for
        // less time per cycle.
        cTimeoutMultiplier: 2,
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
/// to pull the PIN, the radio handle, and to push diagnostic events
/// back into the UI.
struct PairContext {
    pin: [u8; 6],
    addr: u64,
    h_radio: HANDLE,
    events: Sender<ScannerEvent>,
}

impl Drop for PairContext {
    fn drop(&mut self) {
        // Close the radio handle when the box is freed (i.e. after
        // `BluetoothAuthenticateDeviceEx` returns and we
        // `Box::from_raw` in `pair`).
        let _ = unsafe { CloseHandle(self.h_radio) };
    }
}

unsafe extern "system" fn auth_callback(
    pv_param: *const c_void,
    p_params: *const BLUETOOTH_AUTHENTICATION_CALLBACK_PARAMS,
) -> BOOL {
    let params = &*p_params;
    let ctx = &*(pv_param as *const PairContext);

    debug!(
        "wiimote auth callback for {}: method = {:?}",
        format_addr_short(ctx.addr),
        params.authenticationMethod
    );

    if params.authenticationMethod != BLUETOOTH_AUTHENTICATION_METHOD_LEGACY {
        warn!(
            "wiimote auth: unexpected method {:?}",
            params.authenticationMethod
        );
        return FALSE;
    }

    let mut response: BLUETOOTH_AUTHENTICATE_RESPONSE = zeroed();
    response.bthAddressRemote = params.deviceInfo.Address;
    response.authMethod = BLUETOOTH_AUTHENTICATION_METHOD_LEGACY;
    response.Anonymous.pinInfo.pin[..6].copy_from_slice(&ctx.pin);
    response.Anonymous.pinInfo.pinLength = 6;
    response.negativeResponse = 0;

    // Use NULL here per MS docs ("uses the radio that received the
    // authentication request"). Passing our `ctx.h_radio` from
    // `BluetoothFindFirstRadio` has been observed to hang indefinitely
    // inside the driver after a manual unpair has perturbed BT state;
    // NULL goes through the same path the BT stack already chose for
    // this auth conversation.
    let err = BluetoothSendAuthenticationResponseEx(HANDLE::default(), &response);

    if err == ERROR_SUCCESS.0 {
        debug!("wiimote auth: PIN response sent");
        TRUE
    } else {
        warn!("wiimote auth: SendAuthenticationResponseEx 0x{:08x}", err);
        let _ = ctx.events.send(ScannerEvent::Error(format!(
            "PIN response failed: 0x{err:08x}"
        )));
        FALSE
    }
}

fn format_addr_short(addr: u64) -> String {
    let b = addr.to_le_bytes();
    format!("{:02X}:{:02X}:{:02X}", b[2], b[1], b[0])
}

fn pair(info: &BLUETOOTH_DEVICE_INFO, events: &Sender<ScannerEvent>) -> Result<(), String> {
    let bytes = unsafe { info.Address.Anonymous.rgBytes };
    // The Wiimote in 1+2 pairing mode expects the BD address bytes
    // in the order they're sent on the wire (LSB first per BT spec).
    // Windows already stores `rgBytes` LSB-first, so we pass it
    // through unchanged.
    let pin: [u8; 6] = bytes;
    let addr = unsafe { info.Address.Anonymous.ullLong };
    debug!(
        "wiimote pair: PIN [{:02x} {:02x} {:02x} {:02x} {:02x} {:02x}]",
        pin[0], pin[1], pin[2], pin[3], pin[4], pin[5]
    );

    // Resolve a real local-radio handle. NULL works for
    // `BluetoothAuthenticateDeviceEx` ("try every radio") but
    // `BluetoothSendAuthenticationResponseEx` returns ERROR_GEN_FAILURE
    // (0x1F) with NULL when other devices are already paired/connected
    // — apparently the BT stack can't disambiguate which radio should
    // route the response. Passing the explicit handle resolves it.
    let radio_params = BLUETOOTH_FIND_RADIO_PARAMS {
        dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
    };
    let mut h_radio: HANDLE = HANDLE::default();
    let h_find = match unsafe { BluetoothFindFirstRadio(&radio_params, &mut h_radio) } {
        Ok(h) => h,
        Err(_) => return Err("no Bluetooth radio found".into()),
    };
    // `h_find` is just the iterator handle; we close it immediately —
    // the radio handle survives until `PairContext` is dropped.
    let _ = unsafe { BluetoothFindRadioClose(h_find) };

    let ctx = Box::into_raw(Box::new(PairContext {
        pin,
        addr,
        h_radio,
        events: events.clone(),
    }));

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
            h_radio,
            &mut info_local,
            None,
            MITMProtectionNotRequired,
        )
    };

    let _ = unsafe { BluetoothUnregisterAuthentication(h_callback) };
    // Drops PairContext, which closes h_radio.
    let _ = unsafe { Box::from_raw(ctx) };

    if auth_err != ERROR_SUCCESS.0 {
        return Err(format!("AuthenticateDeviceEx 0x{auth_err:08x}"));
    }
    Ok(())
}

fn enable_hid_service(
    info: &BLUETOOTH_DEVICE_INFO,
    events: &Sender<ScannerEvent>,
) -> Result<(), String> {
    let mut info_local = *info;
    let addr = unsafe { info.Address.Anonymous.ullLong };
    let result = with_radio(|h_radio| unsafe {
        // Refresh the cached BT registry record for this device. The
        // info returned by inquiry can be missing post-pair service
        // entries, which can cause SetServiceState to choke with
        // ERROR_INVALID_PARAMETER.
        let refresh_rc = BluetoothGetDeviceInfo(h_radio, &mut info_local);
        if refresh_rc != ERROR_SUCCESS.0 {
            debug!("GetDeviceInfo 0x{:08x}", refresh_rc);
        }

        // Best-effort diagnostic: log the registered service list so
        // debug builds can correlate SDP-cache state with the
        // SetServiceState outcome.
        let _ = enumerate_services_lookup_hid(h_radio, &info_local);

        let svc_rc = BluetoothSetServiceState(
            h_radio,
            &info_local,
            &HID_SERVICE_GUID,
            BLUETOOTH_SERVICE_ENABLE,
        );
        // ERROR_INVALID_PARAMETER (0x57) here is the canonical Wii
        // Remote Plus signature: Windows holds onto stale post-pair
        // SDP entries from the previous power cycle, and the only
        // recovery is to unpair-then-repair. Surface it so the daemon
        // can auto-recover during a manual scan window.
        if svc_rc == ERROR_INVALID_PARAMETER.0 {
            let _ = events.send(ScannerEvent::SdpCacheStale { addr });
        }
        svc_rc
    })?;
    if result != ERROR_SUCCESS.0 {
        return Err(format!("SetServiceState 0x{result:08x}"));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
enum ServiceLookup {
    HidPresent,
    MissingHid,
    Empty,
    Failed,
}

/// Walks the installed-services list of a paired BT device and reports
/// whether the HID service GUID is among them. Sized dynamically so
/// devices with >8 services aren't truncated (B5).
unsafe fn enumerate_services_lookup_hid(
    h_radio: HANDLE,
    info: &BLUETOOTH_DEVICE_INFO,
) -> ServiceLookup {
    // First call with NULL buffer to learn the real count.
    let mut count: u32 = 0;
    let probe_rc = BluetoothEnumerateInstalledServices(h_radio, info, &mut count, None);
    if probe_rc != ERROR_SUCCESS.0 && count == 0 {
        debug!("EnumerateInstalledServices probe rc=0x{:08x}", probe_rc);
        return ServiceLookup::Failed;
    }
    if count == 0 {
        return ServiceLookup::Empty;
    }
    let mut services: Vec<GUID> = vec![GUID::from_u128(0); count as usize];
    let rc = BluetoothEnumerateInstalledServices(
        h_radio,
        info,
        &mut count,
        Some(services.as_mut_ptr()),
    );
    if rc != ERROR_SUCCESS.0 {
        debug!("EnumerateInstalledServices 0x{:08x}", rc);
        return ServiceLookup::Failed;
    }
    let n = (count as usize).min(services.len());
    for s in &services[..n] {
        debug!(
            "installed service: {:08x}-{:04x}-{:04x}",
            s.data1, s.data2, s.data3
        );
        if s.data1 == 0x0000_1124 && s.data2 == 0x0000 && s.data3 == 0x1000 {
            return ServiceLookup::HidPresent;
        }
    }
    ServiceLookup::MissingHid
}

/// Unpair a device from the local Bluetooth radio. Used by the daemon
/// when the user clicks "Forget" — without this the next BT inquiry
/// would re-discover the still-paired device and re-add it.
pub fn unpair(addr: u64) -> Result<(), String> {
    let address = windows::Win32::Devices::Bluetooth::BLUETOOTH_ADDRESS {
        Anonymous:
            windows::Win32::Devices::Bluetooth::BLUETOOTH_ADDRESS_0 { ullLong: addr },
    };
    let rc = unsafe { BluetoothRemoveDevice(&address) };
    if rc == ERROR_SUCCESS.0 {
        Ok(())
    } else {
        Err(format!("BluetoothRemoveDevice 0x{rc:08x}"))
    }
}

/// Open the local Bluetooth radio, run `f` with its handle, then close
/// the handle. NULL/default doesn't work reliably for several BT APIs
/// when the system has more than one paired device — the stack can't
/// disambiguate which radio routes the call.
fn with_radio<T>(f: impl FnOnce(HANDLE) -> T) -> Result<T, String> {
    let radio_params = BLUETOOTH_FIND_RADIO_PARAMS {
        dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
    };
    let mut h_radio: HANDLE = HANDLE::default();
    let h_find = match unsafe { BluetoothFindFirstRadio(&radio_params, &mut h_radio) } {
        Ok(h) => h,
        Err(_) => return Err("no Bluetooth radio found".into()),
    };
    let _ = unsafe { BluetoothFindRadioClose(h_find) };
    let result = f(h_radio);
    let _ = unsafe { CloseHandle(h_radio) };
    Ok(result)
}
