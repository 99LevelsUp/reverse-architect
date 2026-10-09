//! What a long run tells the operator while it works (U-A).
//!
//! Measured on the R17 target: a 14,396-file repository takes ~18 minutes, and
//! until now it printed nothing at all until the very end. Eighteen minutes of
//! silence is indistinguishable from a hang, and the operator has no basis for
//! deciding whether to wait or to kill it.
//!
//! ## Why the library only counts, and never prints
//!
//! Everything here is counts and names. The clock, the terminal, the escape
//! codes and the arithmetic that turns a rate into an ETA all live in the
//! binary. Two reasons, and the first is not style:
//!
//! - **R18.** The corpus must be byte-identical across runs and machines. A
//!   library that reads a clock is a library that can leak a timestamp into an
//!   artifact by accident. It cannot leak what it cannot read.
//! - The library is also used by tests and by the enrich path, neither of which
//!   wants a terminal.
//!
//! Progress goes to stderr and never to a file the corpus contains, so it cannot
//! affect a single byte of output.
//!
//! ## The shape that survives a log
//!
//! [`crate::progress`] deliberately does not decide between a redrawn line and a
//! line per tick — that is the reporter's call, because it depends on whether
//! stderr is a terminal. Measured against lean-ctx 3.9.18, which an agent may
//! have wrapped around the run: it has no progress-specific rule, only a generic
//! "keep the first five lines and the last five" elision of a repetitive block.
//! A carriage-returned line is ONE physical line, so forty ticks survive as one;
//! forty newline-terminated ticks survive as ten plus an omission notice. The
//! terminal reporter therefore redraws, and the file reporter summarizes.

/// A run reporting what it is doing, in counts the caller can render however it
/// likes.
///
/// Every method takes `&self`: a reporter owns whatever mutable state it needs
/// behind its own cell, so a progress handle can be shared without forcing the
/// whole pipeline to thread a `&mut`.
pub trait Progress {
    /// A phase began. `total` is the denominator when the phase knows one up
    /// front, and `None` when it does not — a reporter must render both.
    ///
    /// Opening a phase while one is in flight FINISHES that one. An extractor
    /// that does more than one pass over the repository says so by opening a
    /// sub-phase per pass, and a reporter must not need to be told twice: with
    /// one phase per extractor, `treesitter` reached its total at 38% of its
    /// real duration and then sat at "done" for another eighty seconds while it
    /// walked every file a second time.
    fn phase(&self, name: &str, total: Option<usize>);

    /// The current phase has learned its denominator.
    ///
    /// Separate from [`Progress::phase`] because the runner opens a phase before
    /// the extractor has counted anything, and only the extractor knows what it
    /// is about to walk. Announcing a second phase to carry the total would open
    /// two and close one, and the reporter would attribute the rest of the run
    /// to the wrong name.
    fn total(&self, total: usize);

    /// `done` units of the current phase are complete. Absolute, not a delta:
    /// a caller that skips units (an unreadable blob, an unsupported language)
    /// stays honest without having to report the skips separately.
    fn tick(&self, done: usize);

    /// The current phase is over. The reporter decides whether that means a
    /// summary line, a newline, or nothing.
    fn phase_done(&self);
}

/// A reporter that says nothing.
///
/// The default everywhere, so a caller that does not want progress — every test,
/// the enrich path, a library consumer — pays nothing and configures nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl Progress for Silent {
    fn phase(&self, _name: &str, _total: Option<usize>) {}
    fn total(&self, _total: usize) {}
    fn tick(&self, _done: usize) {}
    fn phase_done(&self) {}
}

/// How often a file-walking phase reports, in files.
///
/// A tick per file over 14,396 files is 14,396 reports, and at that rate the
/// terminal write costs more than the work between two of them. Every 64 keeps
/// a redrawn line visibly moving without the reporting becoming the workload.
/// The final count is always reported exactly, whatever the interval.
pub const TICK_EVERY: usize = 64;

/// Whether a phase that has completed `done` units should report now.
///
/// Extracted so the two file-walking extractors cannot drift apart on it, and so
/// the "always report the last one" rule lives in one place rather than being
/// remembered at each call site.
pub fn should_tick(done: usize, total: usize) -> bool {
    done == total || done.is_multiple_of(TICK_EVERY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_unit_always_reports_however_the_interval_falls() {
        // 100 is not a multiple of 64, and a phase that never reported its own
        // completion would leave a redrawn line stuck at 64/100 forever.
        assert!(should_tick(100, 100));
        assert!(should_tick(64, 100));
        assert!(!should_tick(65, 100));
        // …including the degenerate phase, which must not divide by zero or
        // leave the line unfinished.
        assert!(should_tick(0, 0));
    }
}
