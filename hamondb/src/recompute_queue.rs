use crate::column::{CellId, ColumnId};
use crossbeam_channel::{SendError, Sender};
use dashmap::DashMap;
use std::any::TypeId;
use std::collections::HashSet;
use std::sync::Arc;

pub(super) struct RecomputeQueue {
    crossbeam_channel_sender: Sender<(TypeId, u64)>,
    queue: Arc<DashMap<(TypeId, u64), HashSet<ColumnId>>>,
}

impl RecomputeQueue {
    fn payload_msg(payload: &Box<dyn std::any::Any + Send>) -> String {
        if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic>".to_string()
        }
    }

    pub(super) fn new(
        callback: Box<dyn Fn(TypeId, u64, HashSet<ColumnId>) + Send + Sync + 'static>,
        num_worker: usize,
    ) -> Self {
        let (crossbeam_channel_sender, crossbeam_channel_receiver) = crossbeam_channel::unbounded();
        let queue: Arc<DashMap<(TypeId, u64), HashSet<ColumnId>>> = Arc::new(DashMap::new());
        let callback = Arc::new(callback);
        for _ in 0..num_worker {
            let receiver = crossbeam_channel_receiver.clone();
            let callback = callback.clone();
            let queue = queue.clone();
            std::thread::spawn(move || {
                loop {
                    match receiver.recv() {
                        Ok(key) => {
                            match queue.remove(&key) {
                                Some(value) => {
                                    let result =
                                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                            || callback(value.0 .0, value.0 .1, value.1),
                                        ));
                                    if let Err(payload) = result {
                                        eprintln!(
                                            "hamondb worker panicked: {:?}",
                                            Self::payload_msg(&payload)
                                        );
                                        std::process::abort();
                                    }
                                }
                                None => {
                                    // already processed
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        RecomputeQueue {
            crossbeam_channel_sender,
            queue,
        }
    }

    pub(super) fn enqueue(&self, cell_id: CellId) -> Result<(), SendError<(TypeId, u64)>> {
        match self.queue.entry((cell_id.table, cell_id.row)) {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                entry.get_mut().insert(cell_id.column);
                Ok(())
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let mut hash = HashSet::new();
                hash.insert(cell_id.column);
                entry.insert(hash);
                self.crossbeam_channel_sender
                    .send((cell_id.table, cell_id.row))
            }
        }
    }
}
