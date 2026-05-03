//! Bridge tracing events into the UI log panel.
//!
//! The daemon and its sub-crates use `tracing::{info, warn, error}`
//! for terminal-visible diagnostics. The eframe UI has its own log
//! panel fed by `UiEvent::Log`. Without this bridge the user has to
//! tail the terminal *and* watch the UI to piece together what's
//! happening — exactly the split the layer below removes.
//!
//! Direct `UiEvent::Log` emissions inside the daemon (the existing
//! `[BT] …` lines) carry target `LOG_TARGET_DIRECT` when they also
//! mirror to tracing, so the layer skips them and we don't double-
//! print in the UI.
//!
//! The sender is installed via `install_ui_log_sender` *after* the
//! daemon thread is up. Events fired before installation are dropped,
//! which is fine — the very first frames of startup are nothing the
//! user needs to see.

use crate::{LogLevel, UiEvent};
use crossbeam_channel::Sender;
use std::sync::OnceLock;
use std::time::SystemTime;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Target string used by the daemon's own UI log emissions when they
/// already write to `events_tx` directly. The layer ignores these to
/// avoid duplicate entries in the UI.
pub const LOG_TARGET_DIRECT: &str = "wiipair::ui_log";

static UI_LOG_TX: OnceLock<Sender<UiEvent>> = OnceLock::new();

/// Wire the running UI's event sender into the tracing layer. Idempotent.
pub fn install_ui_log_sender(tx: Sender<UiEvent>) {
    let _ = UI_LOG_TX.set(tx);
}

fn sender() -> Option<&'static Sender<UiEvent>> {
    UI_LOG_TX.get()
}

/// Tracing `Layer` that mirrors info/warn/error events from
/// `wiimote_*` and `wiipair*` modules to the UI log panel.
pub struct UiLogLayer;

impl<S> Layer<S> for UiLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let Some(tx) = sender() else { return };
        let metadata = event.metadata();
        let target = metadata.target();
        if target == LOG_TARGET_DIRECT {
            return;
        }
        if !target.starts_with("wiimote_") && !target.starts_with("wiipair") {
            return;
        }
        let level = match *metadata.level() {
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::ERROR => LogLevel::Error,
            // DEBUG/TRACE stay terminal-only — they're too noisy to
            // mix into a 256-line scrollback the user is reading.
            _ => return,
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        if visitor.message.is_empty() {
            return;
        }
        let _ = tx.send(UiEvent::Log {
            at: SystemTime::now(),
            level,
            message: visitor.message,
        });
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}
