use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();
static BOOT: OnceLock<Instant> = OnceLock::new();
static LAST_MARK_MS: AtomicU64 = AtomicU64::new(0);
static STAT_CALLS: AtomicU32 = AtomicU32::new(0);

pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("EZICODE_PERF")
            .or_else(|| std::env::var_os("OLOVA_PERF"))
            .is_some_and(|v| v != "0" && !v.is_empty())
    })
}

fn boot() -> &'static Instant {
    BOOT.get_or_init(Instant::now)
}

/// Milliseconds since the first perf call this process made.
pub fn elapsed_ms() -> f64 {
    boot().elapsed().as_secs_f64() * 1000.0
}

/// Report a milestone: total ms since boot, plus ms since the previous mark.
pub fn mark(label: &str) {
    if !enabled() {
        return;
    }
    let now = elapsed_ms();
    let prev = LAST_MARK_MS.swap(now as u64, Ordering::Relaxed);
    eprintln!(
        "[perf] {label}: {now:.1} ms total (+{:.1})",
        now - prev as f64
    );
}

/// RAII span. Reports elapsed time when the guard is dropped.
///
/// `label` is `'static` on purpose: it is a fixed site name, not a formatted

pub struct Span {
    label: &'static str,
    start: Instant,
    report: bool,
}

impl Span {
    /// Start timing `label`. Zero cost (beyond a branch) when disabled.
    pub fn start(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
            report: enabled(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.report {
            eprintln!(
                "[perf] {}: {:.1} ms",
                self.label,
                self.start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

pub fn span(label: &'static str) -> Span {
    Span::start(label)
}

/// Record one filesystem probe (only counted while perf output is enabled).
pub fn note_stat() {
    if enabled() {
        STAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Filesystem probes recorded so far.
pub fn stat_count() -> u32 {
    STAT_CALLS.load(Ordering::Relaxed)
}

/// Zero the probe counter. Not thread-safe against concurrent probes — intended
/// for single-threaded benchmarks.
pub fn reset_stats() {
    STAT_CALLS.store(0, Ordering::Relaxed);
}
