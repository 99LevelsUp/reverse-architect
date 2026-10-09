//! The terminal side of [`reverse_architect::progress`] (U-A).
//!
//! Lives in the binary, not the library: this is the half that reads a clock,
//! writes escape sequences and decides what a human wants to look at. The
//! library only counts (see that module for why the clock must not be in it).
//!
//! ## Two renderings, chosen by what stderr is
//!
//! - **A terminal** gets one line per phase, redrawn in place with `\r`. It
//!   never scrolls, and it is also the shape that survives being captured: a
//!   redrawn line is ONE physical line, so a wrapper that elides repetitive
//!   output has nothing to elide. Measured against lean-ctx 3.9.18 — forty
//!   redraws arrive as one line, forty newline-terminated ticks arrive as ten
//!   plus an omission notice.
//! - **Anything else** — a pipe, a file, CI — gets one summary line per phase
//!   when the phase ends, and nothing in between. A `\r` written to a file
//!   produces a single unreadable line thousands of columns wide.
//!
//! ## The estimate
//!
//! Extrapolated from the phase's own rate so far: elapsed divided by the
//! fraction done. It is deliberately naive, and it is labelled `~` because a
//! walk over source files is not uniform — a directory of generated code runs
//! far faster per file than one of hand-written classes. An estimate that is
//! honestly rough beats no estimate at all when the alternative is eighteen
//! minutes of silence, which is what this replaces.

use reverse_architect::progress::Progress;
use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::time::Instant;

/// One phase in flight.
struct Phase {
    name: String,
    total: Option<usize>,
    started: Instant,
    /// Widest line drawn so far, so a shorter one can be padded over its tail
    /// rather than leaving the previous line's characters on screen.
    drawn: usize,
}

/// Renders progress to stderr for a person watching it happen.
pub struct TerminalProgress {
    /// `\r` redraws only make sense on a terminal.
    interactive: bool,
    phase: RefCell<Option<Phase>>,
}

impl TerminalProgress {
    pub fn new() -> TerminalProgress {
        TerminalProgress {
            interactive: std::io::stderr().is_terminal(),
            phase: RefCell::new(None),
        }
    }

    /// Write and flush, ignoring the error.
    ///
    /// Progress is not the work. A closed pipe — `| head`, a killed terminal —
    /// must not turn an eighteen-minute run into a failure at minute seventeen.
    fn draw(&self, line: &str) {
        let mut err = std::io::stderr();
        let _ = write!(err, "{line}");
        let _ = err.flush();
    }
}

impl Default for TerminalProgress {
    fn default() -> Self {
        TerminalProgress::new()
    }
}

impl Progress for TerminalProgress {
    fn phase(&self, name: &str, total: Option<usize>) {
        // A phase already in flight is finished first. That lets a long
        // extractor open sub-phases of its own — `treesitter` walks every file
        // twice, and before this the line reached its total at 38% of the
        // phase's real duration and then sat there. A progress line that says
        // "done" and keeps working for another eighty seconds is the failure
        // this whole unit exists to prevent.
        if self.phase.borrow().is_some() {
            self.phase_done();
        }
        *self.phase.borrow_mut() = Some(Phase {
            name: name.to_string(),
            total,
            started: Instant::now(),
            drawn: 0,
        });
        if self.interactive {
            // Say the phase started before any unit completes: a phase that
            // spends thirty seconds building an index before its first tick
            // would otherwise look like the previous one hanging.
            let line = format!("  {name} …");
            self.draw(&format!("\r{line}"));
            if let Some(phase) = self.phase.borrow_mut().as_mut() {
                phase.drawn = line.len();
            }
        }
    }

    fn total(&self, total: usize) {
        if let Some(phase) = self.phase.borrow_mut().as_mut() {
            phase.total = Some(total);
        }
    }

    fn tick(&self, done: usize) {
        if !self.interactive {
            return;
        }
        let mut slot = self.phase.borrow_mut();
        let Some(phase) = slot.as_mut() else {
            return;
        };
        let elapsed = phase.started.elapsed().as_secs_f64();
        let line = match phase.total {
            Some(total) if total > 0 => {
                let fraction = done as f64 / total as f64;
                format!(
                    "  {} {done}/{total} · {:.0}% · {} elapsed · ~{} left",
                    phase.name,
                    fraction * 100.0,
                    clock(elapsed),
                    clock(remaining(elapsed, fraction)),
                )
            }
            _ => format!("  {} {done} · {} elapsed", phase.name, clock(elapsed)),
        };
        let pad = phase.drawn.saturating_sub(line.len());
        phase.drawn = line.len();
        self.draw(&format!("\r{line}{:pad$}", "", pad = pad));
    }

    fn phase_done(&self) {
        let Some(phase) = self.phase.borrow_mut().take() else {
            return;
        };
        let took = clock(phase.started.elapsed().as_secs_f64());
        let summary = match phase.total {
            Some(total) => format!("  {} {total}/{total} in {took}", phase.name),
            None => format!("  {} in {took}", phase.name),
        };
        // Newline-terminated, so it survives both a redraw and any wrapper that
        // keeps only the last lines of a block. This is the line a reader still
        // has after the run — the per-phase timing of the whole pipeline.
        let pad = phase.drawn.saturating_sub(summary.len());
        self.draw(&format!(
            "{}{summary}{:pad$}\n",
            if self.interactive { "\r" } else { "" },
            "",
            pad = pad
        ));
    }
}

/// Seconds as `h:mm:ss`, or `m:ss` under an hour. Never a bare float: the
/// reader is estimating whether to wait, and `632.4` makes them do arithmetic.
fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "?".to_string();
    }
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    match h {
        0 => format!("{m}:{s:02}"),
        _ => format!("{h}:{m:02}:{s:02}"),
    }
}

/// How long the rest of a phase should take at the rate it has managed so far.
///
/// Zero progress gives no basis for an estimate, and dividing by it would print
/// `inf` — [`clock`] renders that as `?`, which is the honest answer.
fn remaining(elapsed: f64, fraction: f64) -> f64 {
    if fraction <= 0.0 {
        return f64::INFINITY;
    }
    (elapsed / fraction - elapsed).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_reads_as_a_clock_not_as_a_float() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(9.4), "0:09");
        assert_eq!(clock(632.0), "10:32");
        assert_eq!(clock(3600.0), "1:00:00");
        assert_eq!(clock(4271.0), "1:11:11");
    }

    #[test]
    fn an_estimate_with_nothing_to_extrapolate_from_says_so() {
        // The first tick of a phase, before any fraction is done: printing
        // `inf` or `0:00` would both be lies, one alarming and one reassuring.
        assert_eq!(clock(remaining(5.0, 0.0)), "?");
        // Half done in ten seconds means about ten more.
        assert_eq!(clock(remaining(10.0, 0.5)), "0:10");
        // Finished means none left, never a negative.
        assert_eq!(clock(remaining(10.0, 1.0)), "0:00");
    }
}
