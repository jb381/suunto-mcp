use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc},
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct Outgoing {
    pub response: Value,
    pub work: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub enum Transport {
    Http { version: &'static str },
    Sse(mpsc::Sender<Outgoing>),
}

struct Session {
    transport: Transport,
    cancellation: CancellationToken,
    last_activity: Instant,
    active: usize,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub struct Sessions {
    entries: Mutex<HashMap<Uuid, Session>>,
    limit: usize,
    idle: Duration,
}

pub struct Lease {
    id: Uuid,
    sessions: Arc<Sessions>,
    pub transport: Transport,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(entry) = self.sessions.entries.lock().unwrap().get_mut(&self.id) {
            entry.active -= 1;
            entry.last_activity = Instant::now();
        }
    }
}

impl Sessions {
    pub fn new(limit: usize, idle: Duration) -> Arc<Self> {
        let sessions = Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            limit,
            idle,
        });
        let weak = Arc::downgrade(&sessions);
        tokio::spawn(async move {
            let mut tick = time::interval(idle.min(Duration::from_secs(30)));
            loop {
                tick.tick().await;
                let Some(sessions) = weak.upgrade() else {
                    break;
                };
                let mut entries = sessions.entries.lock().unwrap();
                sessions.prune(&mut entries);
            }
        });
        sessions
    }

    fn prune(&self, entries: &mut HashMap<Uuid, Session>) {
        let now = Instant::now();
        entries.retain(|_, entry| {
            entry.active != 0 || now.duration_since(entry.last_activity) < self.idle
        });
    }

    pub fn insert(self: &Arc<Self>, transport: Transport) -> Option<(Uuid, CancellationToken)> {
        let mut entries = self.entries.lock().unwrap();
        self.prune(&mut entries);
        if entries.len() >= self.limit {
            return None;
        }
        let id = Uuid::new_v4();
        let cancellation = CancellationToken::new();
        entries.insert(
            id,
            Session {
                transport,
                cancellation: cancellation.clone(),
                last_activity: Instant::now(),
                active: 0,
            },
        );
        Some((id, cancellation))
    }

    pub fn lease(self: &Arc<Self>, id: Uuid) -> Option<Lease> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(&id)?;
        if entry.active == 0 && entry.last_activity.elapsed() >= self.idle {
            entries.remove(&id);
            return None;
        }
        entry.active += 1;
        Some(Lease {
            id,
            sessions: self.clone(),
            transport: entry.transport.clone(),
        })
    }

    pub fn remove(&self, id: Uuid) {
        self.entries.lock().unwrap().remove(&id);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn reap_idle_sessions_and_leave_active_work_alone() {
        let sessions = Sessions::new(2, Duration::from_secs(30));
        let transport = Transport::Http {
            version: "2025-11-25",
        };
        let (id, _) = sessions.insert(transport.clone()).unwrap();
        let (_, idle) = sessions.insert(transport.clone()).unwrap();
        let lease = sessions.lease(id).unwrap();
        assert!(sessions.insert(transport.clone()).is_none());

        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        assert!(idle.is_cancelled());
        assert_eq!(sessions.len(), 1);
        drop(lease);

        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        assert_eq!(sessions.len(), 0);
        assert!(sessions.insert(transport).is_some());
    }
}
