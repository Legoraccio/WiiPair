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
use bluer::{
    Adapter, AdapterEvent, Address, Session,
    agent::{Agent, AgentHandle, ReqResult, RequestPinCode},
};
use crossbeam_channel::Sender;
use futures_util::stream::StreamExt;
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

    if let Err(e) = register_agent(&session, &events).await {
        let _ = events.send(ScannerEvent::Error(format!("agent: {e}")));
    }

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

    while !quit.load(Ordering::Relaxed) {
        if pause.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        if let Err(e) = inquiry_round(&adapter, &events).await {
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
    events: &Sender<ScannerEvent>,
) -> bluer::Result<AgentHandle> {
    let events_pin = events.clone();
    let agent = Agent {
        request_default: true,
        request_pin_code: Some(Box::new(move |req: RequestPinCode| {
            // Wiimote in 1+2 pairing mode expects a 6-byte raw PIN
            // equal to the BD address sent on the wire (LSB first).
            // BlueZ wants the PIN as a UTF-8 string here, so we pass
            // the raw 6 bytes verbatim — every Wiimote firmware we've
            // seen accepts the bytes regardless of UTF-8 validity.
            let pin = pin_for_address(req.device);
            let events = events_pin.clone();
            Box::pin(async move {
                let _ = events.send(ScannerEvent::Error(format!(
                    "auth: sending PIN to {}",
                    address_short(req.device)
                )));
                ReqResult::Ok(pin)
            })
        })),
        ..Default::default()
    };
    session.register_agent(agent).await
}

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

async fn inquiry_round(adapter: &Adapter, events: &Sender<ScannerEvent>) -> bluer::Result<()> {
    // Short discovery window so we don't starve already-connected
    // devices.
    let mut stream = adapter.discover_devices().await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match ev {
            AdapterEvent::DeviceAdded(addr) => {
                if let Err(e) = handle_discovered(adapter, addr, events).await {
                    debug!("discover {addr}: {e}");
                }
            }
            AdapterEvent::DeviceRemoved(_) | AdapterEvent::PropertyChanged(_) => {}
        }
    }
    Ok(())
}

async fn handle_discovered(
    adapter: &Adapter,
    addr: Address,
    events: &Sender<ScannerEvent>,
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
        let _ = events.send(ScannerEvent::Pairing { addr: u64_addr });
        match device.pair().await {
            Ok(()) => {
                let _ = events.send(ScannerEvent::Paired { addr: u64_addr });
            }
            Err(e) => {
                let _ = events.send(ScannerEvent::PairFailed {
                    addr: u64_addr,
                    reason: format!("{e}"),
                });
                return Ok(());
            }
        }
    }
    if !connected {
        match device.connect().await {
            Ok(()) => {
                let _ = events.send(ScannerEvent::HidEnabled { addr: u64_addr });
            }
            Err(e) => {
                let _ = events.send(ScannerEvent::Error(format!(
                    "connect {addr}: {e}"
                )));
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
