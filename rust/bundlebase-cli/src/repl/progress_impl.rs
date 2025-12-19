//! Progress tracking implementation for the REPL using indicatif.
//!
//! This module provides visual progress bars in the terminal for long-running
//! Bundlebase operations when using the REPL.

use bundlebase::progress::{ProgressId, ProgressTracker};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Progress tracker that displays terminal progress bars using indicatif.
///
/// This tracker manages multiple concurrent progress bars and supports both
/// determinate (known total) and indeterminate (spinner) progress.
pub struct IndicatifTracker {
    /// Multi-progress manager for handling multiple concurrent progress bars
    multi: Arc<MultiProgress>,
    /// Active progress bars keyed by ProgressId
    bars: Arc<Mutex<HashMap<ProgressId, ProgressBar>>>,
}

impl IndicatifTracker {
    /// Create a new indicatif tracker.
    pub fn new() -> Self {
        Self {
            multi: Arc::new(MultiProgress::new()),
            bars: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a progress bar style for determinate progress.
    fn determinate_style() -> ProgressStyle {
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-")
    }

    /// Create a spinner style for indeterminate progress.
    fn spinner_style() -> ProgressStyle {
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner())
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
    }
}

impl Default for IndicatifTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressTracker for IndicatifTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();

        let bar = if let Some(total_val) = total {
            // Determinate progress: show progress bar
            let pb = self.multi.add(ProgressBar::new(total_val));
            pb.set_style(Self::determinate_style());
            pb.set_message(operation.to_string());
            pb
        } else {
            // Indeterminate progress: show spinner
            let pb = self.multi.add(ProgressBar::new_spinner());
            pb.set_style(Self::spinner_style());
            pb.set_message(operation.to_string());
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            pb
        };

        self.bars.lock().insert(id, bar);
        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        if let Some(bar) = self.bars.lock().get(&id) {
            // Update position
            bar.set_position(current);

            // Update message if provided
            if let Some(msg) = message {
                // Append status message to operation name
                let current_msg = bar.message();
                let base_msg = current_msg.split(" - ").next().unwrap_or(&current_msg);
                bar.set_message(format!("{} - {}", base_msg, msg));
            }
        }
    }

    fn finish(&self, id: ProgressId) {
        if let Some(bar) = self.bars.lock().remove(&id) {
            bar.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indicatif_tracker_basic() {
        let tracker = IndicatifTracker::new();

        let id = tracker.start("Test operation", Some(100));
        tracker.update(id, 50, Some("Halfway"));
        tracker.update(id, 100, None);
        tracker.finish(id);
    }

    #[test]
    fn test_indicatif_tracker_indeterminate() {
        let tracker = IndicatifTracker::new();

        let id = tracker.start("Loading...", None);
        tracker.update(id, 1, Some("Step 1"));
        tracker.update(id, 2, Some("Step 2"));
        tracker.finish(id);
    }

    #[test]
    fn test_indicatif_tracker_multiple() {
        let tracker = IndicatifTracker::new();

        let id1 = tracker.start("Operation A", Some(100));
        let id2 = tracker.start("Operation B", None);

        tracker.update(id1, 50, None);
        tracker.update(id2, 1, Some("Processing"));

        tracker.finish(id1);
        tracker.finish(id2);
    }
}
