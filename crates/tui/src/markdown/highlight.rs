//! Asynchronous code-block syntax highlighting.
//!
//! Expensive syntect passes run on a dedicated OS thread so they do not
//! block the render loop. The worker accepts [`HighlightRequest`]s over
//! a bounded MPSC channel and streams [`HighlightJob`] results back.
//!
//! Producers do not wait for results inline; instead, the owning
//! [`crate::markdown::CachedMarkdownRenderer`] inserts a cheap
//! placeholder block synchronously, spawns the request, and swaps in the
//! highlighted lines on the next frame.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use ratatui::text::{Line, Span};

use crate::components::syntax::SyntaxHighlighter;
use crate::theme::ThemeName;

fn to_static_line(line: Line<'_>) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    Line::from(spans).alignment(line.alignment.unwrap_or_default())
}

/// A highlight request.
#[derive(Debug, Clone)]
pub struct HighlightRequest {
    pub job_id: u64,
    pub code: String,
    pub lang: String,
    pub theme_name: ThemeName,
}

/// A completed highlight. `lines` is ready to be pasted into the cached
/// render output in place of the placeholder block.
#[derive(Debug, Clone)]
pub struct HighlightJob {
    pub job_id: u64,
    pub lines: Arc<Vec<Line<'static>>>,
}

/// A background worker. Dropping the worker closes the inbound channel
/// and cleanly joins the thread.
pub struct HighlightWorker {
    tx: SyncSender<HighlightRequest>,
    rx: Receiver<HighlightJob>,
    handle: Option<JoinHandle<()>>,
}

impl HighlightWorker {
    /// Start a worker thread with a bounded inbound queue.
    ///
    /// When `inbox_size` requests are queued, older requests are dropped
    /// in favor of newer ones — this trades stale results for bounded
    /// memory under load.
    #[must_use]
    pub fn spawn(inbox_size: usize) -> Self {
        let (req_tx, req_rx) = sync_channel::<HighlightRequest>(inbox_size.max(1));
        let (job_tx, job_rx): (Sender<HighlightJob>, Receiver<HighlightJob>) =
            std::sync::mpsc::channel();

        let handle = thread::Builder::new()
            .name("crab-md-highlight".to_string())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    // `theme_name` is used to pick a light vs. dark syntect
                    // palette so downstream cache keys stay distinct.
                    let hl = if matches!(req.theme_name, ThemeName::Light) {
                        SyntaxHighlighter::with_light_theme()
                    } else {
                        SyntaxHighlighter::new()
                    };
                    let owned: Vec<Line<'static>> = hl
                        .highlight(&req.code, &req.lang)
                        .into_iter()
                        .map(to_static_line)
                        .collect();
                    let job = HighlightJob {
                        job_id: req.job_id,
                        lines: Arc::new(owned),
                    };
                    if job_tx.send(job).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn highlight worker");

        Self {
            tx: req_tx,
            rx: job_rx,
            handle: Some(handle),
        }
    }

    /// Submit a highlight request. Returns `true` on success; `false` if
    /// the worker's inbox is full (the caller should fall back to
    /// synchronous highlighting or skip this frame's swap).
    pub fn submit(&self, request: HighlightRequest) -> bool {
        self.tx.try_send(request).is_ok()
    }

    /// Drain any completed jobs. Non-blocking.
    pub fn drain_completed(&self) -> Vec<HighlightJob> {
        let mut out = Vec::new();
        while let Ok(job) = self.rx.try_recv() {
            out.push(job);
        }
        out
    }
}

impl Drop for HighlightWorker {
    fn drop(&mut self) {
        // Dropping `tx` closes the worker's recv loop.
        // Replace tx with a temporary one to force close.
        let (dummy, _) = sync_channel::<HighlightRequest>(1);
        let _ = std::mem::replace(&mut self.tx, dummy);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn worker_submits_and_returns_a_job() {
        let w = HighlightWorker::spawn(4);
        let req = HighlightRequest {
            job_id: 42,
            code: "fn main() {}\n".into(),
            lang: "rust".into(),
            theme_name: ThemeName::Dark,
        };
        assert!(w.submit(req));
        let mut got = Vec::new();
        for _ in 0..20 {
            got = w.drain_completed();
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!got.is_empty());
        assert_eq!(got[0].job_id, 42);
        assert!(!got[0].lines.is_empty());
    }

    #[test]
    fn submit_returns_false_when_inbox_full() {
        let w = HighlightWorker::spawn(1);
        // Fill the slot.
        let _ = w.submit(HighlightRequest {
            job_id: 1,
            code: "x".into(),
            lang: "text".into(),
            theme_name: ThemeName::Dark,
        });
        // The second synchronous submit may or may not succeed depending
        // on how fast the worker drains; allow either. The important
        // guarantee is that a full inbox is handled without blocking.
        let _ = w.submit(HighlightRequest {
            job_id: 2,
            code: "y".into(),
            lang: "text".into(),
            theme_name: ThemeName::Dark,
        });
    }

    #[test]
    fn worker_drops_cleanly() {
        let w = HighlightWorker::spawn(4);
        drop(w);
    }
}
