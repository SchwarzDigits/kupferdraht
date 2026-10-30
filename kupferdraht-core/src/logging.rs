// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Kupferdraht's logs reach the app through a listener (Step 117, gap G11).
//!
//! Kupferdraht logs through `tracing`; the product decides where those logs
//! go. [`init_logging`] installs Kupferdraht's subscriber and hands every event
//! of Kupferdraht's own targets (`kupferdraht::…`) at a level or above to the app's
//! [`LogListener`]. Without a listener Kupferdraht stays silent — it writes to
//! no console or stream of its own. Events of dependencies are not
//! forwarded.

use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use crate::error::KupferdrahtError;

/// The severity of a log event — matrix-rust-sdk's vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, uniffi::Enum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            Level::TRACE => Self::Trace,
        }
    }
}

/// One log event of Kupferdraht.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LogRecord {
    pub level: LogLevel,
    /// The module that logged it (`kupferdraht::storage::remote::server`, …).
    pub target: String,
    /// The event's message, its other fields appended as ` key=value`.
    pub message: String,
}

/// Where the app wants Kupferdraht's logs: Kotlin's logcat, Swift's `os_log`,
/// the browser's console or an own logging pipeline. Called on the thread
/// that logged — return fast; a slow listener slows Kupferdraht down.
#[uniffi::export(with_foreign)]
pub trait LogListener: Send + Sync {
    fn on_log(&self, record: LogRecord);
}

/// The installed subscriber's sink; `None` until the first
/// [`init_logging`].
static INSTALLED: Mutex<Option<Arc<LogSink>>> = Mutex::new(None);

/// Hand Kupferdraht's log events at `level` or above to `listener`. The first
/// call installs Kupferdraht's `tracing` subscriber; a later call replaces the
/// level and the listener.
///
/// # Errors
/// `InvalidInput` when another `tracing` subscriber is installed already —
/// an app that brings its own keeps it.
#[uniffi::export]
pub fn init_logging(
    level: LogLevel,
    listener: Arc<dyn LogListener>,
) -> Result<(), KupferdrahtError> {
    let mut installed = INSTALLED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(sink) = installed.as_ref() {
        sink.set(level, listener);
        return Ok(());
    }
    let sink = LogSink::new(level, listener);
    tracing::subscriber::set_global_default(subscriber(Arc::clone(&sink))).map_err(|_| {
        KupferdrahtError::InvalidInput("another tracing subscriber is installed already".into())
    })?;
    *installed = Some(sink);
    debug_assert!(installed.is_some(), "the sink is installed");
    Ok(())
}

/// The level and listener the subscriber forwards to — shared by the
/// installed subscriber and [`init_logging`].
pub(crate) struct LogSink {
    state: RwLock<(LogLevel, Arc<dyn LogListener>)>,
}

impl LogSink {
    pub(crate) fn new(level: LogLevel, listener: Arc<dyn LogListener>) -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new((level, listener)),
        })
    }

    /// Replace the level and the listener.
    pub(crate) fn set(&self, level: LogLevel, listener: Arc<dyn LogListener>) {
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = (level, listener);
    }

    /// Whether an event at `level` reaches the listener.
    fn forwards(&self, level: LogLevel) -> bool {
        level <= self.state.read().unwrap_or_else(PoisonError::into_inner).0
    }

    /// Hand `record` to the listener — outside the lock, so a listener
    /// that logs again cannot deadlock.
    fn forward(&self, record: LogRecord) {
        let listener = Arc::clone(&self.state.read().unwrap_or_else(PoisonError::into_inner).1);
        listener.on_log(record);
    }
}

/// Kupferdraht's subscriber forwarding to `sink`.
pub(crate) fn subscriber(sink: Arc<LogSink>) -> Box<dyn Subscriber + Send + Sync> {
    Box::new(tracing_subscriber::registry().with(ListenerLayer { sink }))
}

/// Whether `target` is one of Kupferdraht's own (the crate is `kupferdraht`).
fn is_kupferdraht(target: &str) -> bool {
    target == "kupferdraht" || target.starts_with("kupferdraht::")
}

/// The layer that hands Kupferdraht's events to the sink.
struct ListenerLayer {
    sink: Arc<LogSink>,
}

impl<S: Subscriber> Layer<S> for ListenerLayer {
    /// A dependency's callsites are never interesting; Kupferdraht's are asked
    /// each time, so a level change applies at once.
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if is_kupferdraht(metadata.target()) {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        is_kupferdraht(metadata.target()) && self.sink.forwards((*metadata.level()).into())
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut text = MessageText::default();
        event.record(&mut text);
        self.sink.forward(LogRecord {
            level: (*metadata.level()).into(),
            target: metadata.target().to_owned(),
            message: text.finish(),
        });
    }
}

/// An event's message, then its other fields as ` key=value`.
#[derive(Default)]
struct MessageText {
    message: String,
    fields: String,
}

impl MessageText {
    fn finish(mut self) -> String {
        self.message.push_str(&self.fields);
        self.message
    }
}

impl Visit for MessageText {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Collects every record it is handed.
    #[derive(Default)]
    struct Collect(Mutex<Vec<LogRecord>>);

    impl LogListener for Collect {
        fn on_log(&self, record: LogRecord) {
            self.0.lock().expect("records").push(record);
        }
    }

    impl Collect {
        fn records(&self) -> Vec<LogRecord> {
            self.0.lock().expect("records").clone()
        }
    }

    fn sink(level: LogLevel) -> (Arc<Collect>, Arc<LogSink>) {
        let collect = Arc::new(Collect::default());
        let sink = LogSink::new(level, collect.clone());
        (collect, sink)
    }

    #[test]
    fn a_warning_reaches_the_listener() {
        let (collect, sink) = sink(LogLevel::Warn);

        tracing::subscriber::with_default(subscriber(sink), || {
            tracing::warn!("storage reconnecting");
        });

        assert_eq!(
            collect.records(),
            vec![LogRecord {
                level: LogLevel::Warn,
                target: "kupferdraht::logging::tests".into(),
                message: "storage reconnecting".into(),
            }]
        );
    }

    #[test]
    fn an_event_below_the_level_is_not_forwarded() {
        let (collect, sink) = sink(LogLevel::Warn);

        tracing::subscriber::with_default(subscriber(sink), || {
            tracing::info!("routine");
            tracing::debug!("detail");
            tracing::error!("broken");
        });

        let levels: Vec<LogLevel> = collect.records().iter().map(|r| r.level).collect();
        assert_eq!(levels, vec![LogLevel::Error]);
    }

    #[test]
    fn the_other_fields_follow_the_message() {
        let (collect, sink) = sink(LogLevel::Info);

        tracing::subscriber::with_default(subscriber(sink), || {
            tracing::info!(attempt = 2, reason = "timeout", "renewal failed");
        });

        assert_eq!(
            collect.records()[0].message,
            "renewal failed attempt=2 reason=\"timeout\""
        );
    }

    /// Only Kupferdraht's own events: a dependency's are not the app's business
    /// and could carry what Kupferdraht's do not.
    #[test]
    fn another_crates_event_is_not_forwarded() {
        let (collect, sink) = sink(LogLevel::Trace);

        tracing::subscriber::with_default(subscriber(sink), || {
            tracing::warn!(target: "hyper::proto", "connection reset");
            tracing::warn!(target: "kupferdraht::sync", "drain failed");
        });

        let targets: Vec<String> = collect.records().into_iter().map(|r| r.target).collect();
        assert_eq!(targets, vec!["kupferdraht::sync".to_owned()]);
    }

    /// A debug switch at runtime: the level changes without a new
    /// subscriber.
    #[test]
    fn the_level_changes_at_runtime() {
        let (collect, sink) = sink(LogLevel::Warn);

        tracing::subscriber::with_default(subscriber(sink.clone()), || {
            tracing::debug!("hidden");
            sink.set(LogLevel::Debug, collect.clone());
            tracing::debug!("shown");
            sink.set(LogLevel::Error, collect.clone());
            tracing::warn!("hidden again");
        });

        let messages: Vec<String> = collect.records().into_iter().map(|r| r.message).collect();
        assert_eq!(messages, vec!["shown".to_owned()]);
    }

    #[test]
    fn a_new_listener_replaces_the_old_one() {
        let (first, sink) = sink(LogLevel::Warn);
        let second = Arc::new(Collect::default());

        tracing::subscriber::with_default(subscriber(sink.clone()), || {
            tracing::warn!("to the first");
            sink.set(LogLevel::Warn, second.clone());
            tracing::warn!("to the second");
        });

        assert_eq!(first.records().len(), 1);
        assert_eq!(second.records()[0].message, "to the second");
    }
}
