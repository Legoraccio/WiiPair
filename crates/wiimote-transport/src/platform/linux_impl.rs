//! Linux Bluetooth backend — BlueZ over DBus via `bluer`.
//!
//! Flow:
//! 1. Spin up a tokio runtime in a dedicated thread.
//! 2. On the runtime, open the default adapter, register a custom
//!    Agent that returns the Wiimote-MAC-reversed PIN for legacy
//!    requests (the same trick the Windows backend uses, just hooked
//!    into BlueZ' agent API instead of `BluetoothSendAuthentication-
//!    ResponseEx`).
//! 3. Subscribe to `DiscoveryFilter` results, look for devices whose
//!    name starts with `Nintendo RVL-CNT-01`, then `Pair` + `Connect`.
//! 4. After connect BlueZ creates `/dev/hidraw*` for the device which
//!    hidapi enumeration picks up — the rest of the daemon is OS-
//!    agnostic from there.
//!
//! The actual scanning runs while `pause_inquiry` is false, mirroring
//! the Windows backend so the daemon's pause-on-connection logic
//! works without changes.

use super::ScannerEvent;
use super::linux_mgmt;
use bluer::{
    Adapter, AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, Session,
    agent::{Agent, AgentHandle, ReqError, RequestPinCode},
};
use crossbeam_channel::Sender;
use futures_util::stream::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, info, warn};

const WIIMOTE_NAME_PREFIX: &str = "Nintendo RVL-CNT-01";

pub struct PlatformScanner {
    events: Sender<ScannerEvent>,
    quit: Arc<AtomicBool>,
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
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = events.send(ScannerEvent::Error(format!(
                            "tokio runtime: {e}"
                        )));
                        return;
                    }
                };
                rt.block_on(scan_loop(events, quit, pause));
            })?;
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

pub fn unpair(addr: u64) -> Result<(), String> {
    // Build a short-lived runtime to invoke BlueZ' RemoveDevice.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio: {e}"))?;
    rt.block_on(async move {
        let session = Session::new().await.map_err(|e| format!("session: {e}"))?;
        let adapter = session
            .default_adapter()
            .await
            .map_err(|e| format!("adapter: {e}"))?;
        let address = u64_to_address(addr);
        adapter
            .remove_device(address)
            .await
            .map_err(|e| format!("remove_device: {e}"))
    })
}

async fn scan_loop(
    events: Sender<ScannerEvent>,
    quit: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
) {
    info!("bluetooth scan loop started");
    let session = match Session::new().await {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(ScannerEvent::Error(format!("DBus session: {e}")));
            return;
        }
    };

    // Hold the AgentHandle for the lifetime of the scan loop. Dropping
    // it immediately would unregister our agent from BlueZ — the
    // initial pair would still succeed via our mgmt PIN helper, but
    // any later reconnect that triggers BlueZ's auth state machine
    // would log `No agent available for request type 0`,
    // `device_request_pin: Operation not permitted`, and finally
    // `control_connect_cb: Permission denied (13)` on L2CAP, leaving
    // the device permanently unreachable until re-paired.
    let _agent_handle = match register_agent(&session, &events).await {
        Ok(handle) => Some(handle),
        Err(e) => {
            let _ = events.send(ScannerEvent::Error(format!("agent: {e}")));
            None
        }
    };

    let adapter = match session.default_adapter().await {
        Ok(a) => a,
        Err(e) => {
            let _ = events.send(ScannerEvent::Error(format!("adapter: {e}")));
            return;
        }
    };

    if let Err(e) = adapter.set_powered(true).await {
        warn!("set_powered: {e}");
    }

    // Wiimote is BR/EDR (Bluetooth Classic), not BLE. Without an
    // explicit transport, bluer's default filter ends up driving LE-
    // only inquiry on some dual-mode adapters (MediaTek in particular),
    // and the Wiimote never appears in DeviceAdded events.
    if let Err(e) = adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::BrEdr,
            ..Default::default()
        })
        .await
    {
        warn!("set_discovery_filter: {e}");
    }

    // Devices that have already failed to pair this session: Page
    // Timeout (Wiimote dropped pairing mode mid-handshake) or Auth
    // Rejected (PIN handshake refused). Skipped on subsequent rounds
    // until WiiPair is restarted, otherwise the loop hammers them
    // every cycle and floods the UI log.
    let mut session_blocklist: HashSet<Address> = HashSet::new();

    // Hand off PIN-code-request handling to a kernel-mgmt-socket
    // helper thread. BlueZ's DBus agent (registered above) can't carry
    // the Wiimote's raw-byte PIN through dbus-daemon's UTF-8 string
    // validator; the mgmt socket can. The agent stays registered
    // because BlueZ requires one for the pairing flow to start, but
    // it's a no-op race-loser once the helper is up.
    let pin_active = linux_mgmt::new_active_set();
    linux_mgmt::start(pin_active.clone(), events.clone());

    while !quit.load(Ordering::Relaxed) {
        if pause.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        if let Err(e) =
            inquiry_round(&adapter, &events, &mut session_blocklist, &pin_active).await
        {
            let _ = events.send(ScannerEvent::Error(format!("inquiry: {e}")));
        }

        // 3 s cooldown between rounds, broken into slices to exit
        // promptly on quit/pause.
        for _ in 0..15 {
            if quit.load(Ordering::Relaxed) || pause.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    debug!("bluetooth scan loop stopped");
}

async fn register_agent(
    session: &Session,
    _events: &Sender<ScannerEvent>,
) -> bluer::Result<AgentHandle> {
    let agent = Agent {
        request_default: true,
        request_pin_code: Some(Box::new(move |req: RequestPinCode| {
            // The Wiimote PIN is 6 raw bytes (BD address, LSB-first)
            // — DBus's UTF-8 string type can't carry that without
            // mangling bytes >= 0x80. The real PIN reply is written
            // straight to the kernel by `linux_mgmt`'s helper thread
            // in microseconds. We deliberately stall this callback
            // for much longer than any pair handshake ever takes so
            // BlueZ never gets a chance to answer the kernel via its
            // own mgmt path with our bogus UTF-8 PIN — which would
            // win the race against our helper and corrupt the link
            // key. By the time we eventually return `Reject` the
            // pair has long since completed (or failed) and the
            // late NEG_REPLY is a no-op the kernel discards.
            Box::pin(async move {
                debug!(
                    "wiimote auth: stalling agent PIN reply for {} (mgmt helper handles it)",
                    address_short(req.device),
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
                Err(ReqError::Rejected)
            })
        })),
        ..Default::default()
    };
    session.register_agent(agent).await
}

// Kept around for documentation: this is the wrong-by-design
// conversion BlueZ's agent path forced us into before we moved PIN
// replies to the kernel mgmt socket. Bytes >= 0x80 get re-encoded
// as 2-byte UTF-8 sequences, so the bytes BlueZ wrote to the kernel
// almost never matched what the Wiimote expected. The real PIN reply
// now lives in `linux_mgmt::send_pin_reply`.
#[allow(dead_code)]
fn pin_for_address(addr: Address) -> String {
    let bytes = addr.0;
    // BlueZ Address is MSB-first; the Wiimote wants LSB-first wire
    // order. Reverse and pass bytes raw — `String::from_utf8_lossy`
    // would mangle the high-bit bytes; we use `as_bytes` casts on the
    // receiving side. Build via from_utf8_unchecked equivalent: the
    // bluer agent API takes `String`, so we can't avoid the round-
    // trip. Many Wiimotes accept the UTF-8 lossy form too, but for
    // strict accuracy we encode bytes 1:1 into the surrogate range.
    let mut s = String::with_capacity(6);
    for b in bytes.iter().rev() {
        s.push(*b as char);
    }
    s
}

async fn inquiry_round(
    adapter: &Adapter,
    events: &Sender<ScannerEvent>,
    blocklist: &mut HashSet<Address>,
    pin_active: &linux_mgmt::ActiveSet,
) -> bluer::Result<()> {
    // Per-round dedup so we don't fire `[BT] discovered` multiple
    // times for the same device on every PropertyChanged tick (RSSI
    // alone can update several times per second).
    let mut seen_in_round: HashSet<Address> = HashSet::new();

    // Process every device BlueZ already knows about *before* the
    // event stream starts. `discover_devices()` doesn't reliably
    // emit DeviceAdded for entries that were already in the BlueZ
    // cache from a prior session — without this pass an unpaired
    // Wiimote that BlueZ saw last time (and whose Name only gets
    // refreshed when the user presses 1+2 mid-round) stays
    // invisible to us forever.
    for addr in adapter.device_addresses().await? {
        if seen_in_round.insert(addr) {
            if let Err(e) =
                handle_discovered(adapter, addr, events, blocklist, pin_active).await
            {
                debug!("discover {addr}: {e}");
            }
        }
    }

    let mut stream = adapter.discover_devices().await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match ev {
            AdapterEvent::DeviceAdded(addr) => {
                if seen_in_round.insert(addr) {
                    if let Err(e) =
                        handle_discovered(adapter, addr, events, blocklist, pin_active).await
                    {
                        debug!("discover {addr}: {e}");
                    }
                }
            }
            // PropertyChanged here is for adapter-level properties
            // (Powered, Discovering, …) and DeviceRemoved we don't
            // act on. Per-device property updates (e.g. Name landing
            // after the user presses 1+2 on a previously-cached
            // device) get picked up on the next round's upfront
            // enumeration of `adapter.device_addresses()` above.
            AdapterEvent::DeviceRemoved(_) | AdapterEvent::PropertyChanged(_) => {}
        }
    }
    Ok(())
}

async fn handle_discovered(
    adapter: &Adapter,
    addr: Address,
    events: &Sender<ScannerEvent>,
    blocklist: &mut HashSet<Address>,
    pin_active: &linux_mgmt::ActiveSet,
) -> bluer::Result<()> {
    let device = adapter.device(addr)?;
    let name = device.name().await?.unwrap_or_default();
    if !name.starts_with(WIIMOTE_NAME_PREFIX) {
        return Ok(());
    }
    let paired = device.is_paired().await.unwrap_or(false);
    let connected = device.is_connected().await.unwrap_or(false);
    let u64_addr = address_to_u64(addr);
    let _ = events.send(ScannerEvent::Discovered {
        addr: u64_addr,
        name: name.clone(),
        paired,
        connected,
    });

    if !paired {
        if blocklist.contains(&addr) {
            return Ok(());
        }
        let _ = events.send(ScannerEvent::Pairing { addr: u64_addr });
        // Tell the mgmt PIN helper we're about to pair this address so
        // it answers the kernel's PIN_CODE_REQUEST. Removed below
        // regardless of pair outcome so a stale entry can't poison
        // unrelated future pair attempts on the same address.
        let wire = linux_mgmt::wire_from_msb(addr.0);
        if let Ok(mut s) = pin_active.lock() {
            s.insert(wire);
        }
        let pair_result = device.pair().await;
        if let Ok(mut s) = pin_active.lock() {
            s.remove(&wire);
        }
        match pair_result {
            Ok(()) => {
                let _ = events.send(ScannerEvent::Paired { addr: u64_addr });
            }
            Err(e) => {
                let raw = format!("{e}");
                // Page Timeout = baseband paging didn't get a reply.
                // Almost always: the user let go of 1+2 and the
                // Wiimote dropped out of pairing mode before BlueZ
                // could finish the handshake. Drop the cached device
                // entry and skip retries this session.
                let reason = if raw.contains("Page Timeout") {
                    let _ = adapter.remove_device(addr).await;
                    blocklist.insert(addr);
                    format!(
                        "{raw} — keep holding 1+2 on the Wiimote (the 4 LEDs \
                         must stay blinking 1→2→3→4) until pairing completes, \
                         then restart WiiPair."
                    )
                } else if raw.contains("Authentication") {
                    blocklist.insert(addr);
                    format!(
                        "{raw} — the Wiimote rejected the PIN handshake. \
                         Restart WiiPair to retry."
                    )
                } else {
                    raw
                };
                let _ = events.send(ScannerEvent::PairFailed {
                    addr: u64_addr,
                    reason,
                });
                return Ok(());
            }
        }
    }
    // Trust the device so BlueZ accepts the Wiimote's later self-
    // initiated reconnects (when the user powers it back on) without
    // prompting an agent for authorization. Without this BlueZ logs
    // `device_request_pin: Operation not permitted` on every reconnect
    // and the L2CAP control socket comes back with Permission denied
    // (13). Idempotent: BlueZ no-ops if already trusted, so it's safe
    // to call on every discovery round.
    if let Err(e) = device.set_trusted(true).await {
        warn!("set_trusted({addr}): {e}");
    }

    if !connected {
        match device.connect().await {
            Ok(()) => {
                let _ = events.send(ScannerEvent::HidEnabled { addr: u64_addr });
            }
            Err(e) => {
                let raw = format!("{e}");
                // Wii Remote Plus quirk: after a power-cycle the
                // controller exposes SDP records that no longer
                // match what BlueZ cached at first pair. The
                // L2CAP HID profile setup then fails with one of
                // a small handful of bluer/BlueZ error strings,
                // depending on exactly *where* the negotiation
                // dies. All of them resolve via the same flow:
                // unpair + re-pair from scratch (the daemon's
                // `SdpCacheStale` handler does this once the
                // user has the manual scan window open).
                let stale = paired
                    && (raw.contains("br-connection-create-socket")
                        || raw.contains("br-connection-canceled")
                        || raw.contains("br-connection-refused")
                        || raw.contains("br-connection-aborted-by-remote"));
                if stale {
                    let _ = events.send(ScannerEvent::SdpCacheStale { addr: u64_addr });
                } else {
                    let _ = events.send(ScannerEvent::Error(format!(
                        "connect {addr}: {e}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn address_to_u64(a: Address) -> u64 {
    // BlueZ stores BD address MSB-first in `[u8; 6]`. The daemon and
    // Win32 backend both speak LSB-first u64 — mirror so cross-OS
    // logs read the same.
    let mut bytes = [0u8; 8];
    for (i, b) in a.0.iter().rev().enumerate() {
        bytes[i] = *b;
    }
    u64::from_le_bytes(bytes)
}

fn u64_to_address(addr: u64) -> Address {
    let bytes = addr.to_le_bytes();
    let mut out = [0u8; 6];
    for (i, b) in bytes.iter().take(6).rev().enumerate() {
        out[i] = *b;
    }
    Address::new(out)
}

fn address_short(a: Address) -> String {
    format!("{:02X}:{:02X}:{:02X}", a.0[0], a.0[1], a.0[2])
}
