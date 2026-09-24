//! Cooperative cancellation for in-flight render requests.
//!
//! Minimal shape: an `Arc<AtomicBool>` flipped by [`CancelToken::cancel`]
//! and polled by workers before they hand a job off to pdfium. Mirrors
//! the primitive the workspace already uses elsewhere without pulling
//! `istmo::CancelToken` into a standalone crate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cheap, clonable cancel flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// Fresh un-cancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark this token cancelled. Cheap; safe from any thread.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// `true` after [`Self::cancel`] has run on any clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}
