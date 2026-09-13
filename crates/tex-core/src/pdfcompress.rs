//! Compress completed pages on one bounded worker, keeping TeX state local.
use std::cell::{OnceCell, RefCell};
use std::sync::mpsc::{self, Receiver, SyncSender};

struct Compressed {
    original: Vec<u8>,
    bytes: Vec<u8>,
}

struct Job {
    original: Vec<u8>,
    result: SyncSender<Compressed>,
}

pub(crate) struct Pending {
    receiver: RefCell<Option<Receiver<Compressed>>>,
    result: OnceCell<Option<Compressed>>,
}

impl Pending {
    pub(crate) fn get(&self, original: &[u8]) -> Option<&[u8]> {
        let result = self
            .result
            .get_or_init(|| self.receiver.borrow_mut().take()?.recv().ok())
            .as_ref()?;
        // PdfDoc pages remain editable through the public API. Never reuse
        // compressed bytes after their content has been changed or reordered.
        (result.original == original).then_some(result.bytes.as_slice())
    }
}

pub(crate) struct Worker {
    sender: Option<SyncSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn new() -> Option<Self> {
        static PARALLEL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*PARALLEL
            .get_or_init(|| std::thread::available_parallelism().is_ok_and(|n| n.get() > 1))
        {
            return None;
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(4);
        let thread = std::thread::Builder::new()
            .name("pdf-compress".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let bytes = crate::pdffile::flate(&job.original);
                    let _ = job.result.send(Compressed {
                        original: job.original,
                        bytes,
                    });
                }
            })
            .ok()?;
        Some(Self {
            sender: Some(sender),
            thread: Some(thread),
        })
    }

    pub(crate) fn submit(&self, bytes: &[u8]) -> Option<Pending> {
        let (result, receiver) = mpsc::sync_channel(1);
        // Saturation falls back to ordinary serialization without stalling TeX.
        self.sender
            .as_ref()?
            .try_send(Job {
                original: bytes.to_vec(),
                result,
            })
            .ok()?;
        Some(Pending {
            receiver: RefCell::new(Some(receiver)),
            result: OnceCell::new(),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn queued_pages_roundtrip_and_reject_stale_content() {
        let Some(worker) = Worker::new() else {
            return;
        };
        let original = b"BT /F1 12 Tf (hello) Tj ET\n".repeat(2000);
        let pending = worker.submit(&original).unwrap();
        let compressed = pending.get(&original).unwrap();
        assert_eq!(compressed, crate::pdffile::flate(&original));
        let mut decoded = Vec::new();
        flate2::read::ZlibDecoder::new(compressed)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, original);
        assert!(pending.get(b"changed").is_none());
        // Dropping a worker drains its queue even with an unread result.
        let _unread = worker.submit(b"another page").unwrap();
        drop(worker);
    }
}
