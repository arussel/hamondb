use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT_FOR_FIRES_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FireRecord {
    pub table: String,
    pub pk: Value,
    pub transition: String,
    pub old: Option<Value>,
    pub new: Option<Value>,
    #[serde(skip)]
    pub seq: u64,
}

pub struct FireLog {
    inner: Mutex<Vec<FireRecord>>,
    cvar: Condvar,
    seq: AtomicU64,
}

impl FireLog {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            cvar: Condvar::new(),
            seq: AtomicU64::new(0),
        }
    }

    pub fn recorder<T>(
        self: &Arc<Self>,
        table: &'static str,
    ) -> impl Fn(&T::PrimaryKey, Option<&T>, Option<&T>) + Send + Sync + 'static
    where
        T: hamondb::Table + Serialize,
        T::PrimaryKey: Serialize,
    {
        let this = self.clone();
        move |pk, old, new| {
            let transition = match (old, new) {
                (Some(old), Some(new)) => "update",
                (None, Some(new)) => "insert",
                (Some(old), None) => "delete",
                (None, None) => unreachable!("listener fired with neither old nor new"),
            };
            let rec = FireRecord {
                table: table.to_string(),
                pk: serde_json::to_value(pk).unwrap(),
                transition: transition.to_string(),
                old: old.map(|r| serde_json::to_value(r).unwrap()),
                new: new.map(|r| serde_json::to_value(r).unwrap()),
                seq: this.seq.fetch_add(1, Ordering::Relaxed),
            };
            let mut g = this.inner.lock();
            g.push(rec);
            this.cvar.notify_all();
        }
    }

    pub fn wait_for(&self, n: usize) {
        let deadline = Instant::now() + WAIT_FOR_FIRES_TIMEOUT;
        let mut g = self.inner.lock();
        while g.len() < n {
            let remaining = match deadline.checked_duration_since(Instant::now()) {
                Some(r) => r,
                None => panic!(
                    "wait_for_fires timed out after {:?}: expected {} fires, observed {}. \
                   Fires so far:\n{}",
                    WAIT_FOR_FIRES_TIMEOUT,
                    n,
                    g.len(),
                    format_fires(&*g),
                ),
            };
            let result = self.cvar.wait_for(&mut g, remaining);
            if result.timed_out() && g.len() < n {
                panic!(
                    "wait_for_fires timed out after {:?}: expected {} fires, observed {}. \
                   Fires so far:\n{}",
                    WAIT_FOR_FIRES_TIMEOUT,
                    n,
                    g.len(),
                    format_fires(&*g),
                );
            }
        }
    }

    pub fn snapshot_canonical(&self) -> Vec<FireRecord> {
        let mut v = self.inner.lock().clone();
        v.sort_by(|a, b| {
            a.table
                .cmp(&b.table)
                .then_with(|| a.pk.to_string().cmp(&b.pk.to_string()))
                .then_with(|| a.seq.cmp(&b.seq))
        });
        v
    }
}

fn format_fires(fires: &[FireRecord]) -> String {
    fires
        .iter()
        .map(|f| format!("  {} pk={} {}", f.table, f.pk, f.transition))
        .collect::<Vec<_>>()
        .join("\n")
}
