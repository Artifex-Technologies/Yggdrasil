//! The transfers waiting their turn: one runs at a time, the next starts when it ends, and finished ones stay listed until
//! they are dismissed (the transfer panel shows them all).

/// Where a job is in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the one before it.
    Queued,
    Running,
    /// Ended (done, stopped or failed); listed until dismissed.
    Finished,
}

pub struct Slot<T> {
    pub id: u64,
    pub phase: Phase,
    pub job: T,
}

/// Jobs in the order they were asked for.
pub struct Queue<T> {
    slots: Vec<Slot<T>>,
    next_id: u64,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), next_id: 1 }
    }
}

impl<T> Queue<T> {
    /// A new job at the end; it waits until the ones before it are done. Its id.
    pub fn push(&mut self, job: T) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.slots.push(Slot { id, phase: Phase::Queued, job });
        id
    }

    /// When nothing runs: the first waiting job, now marked running, to be started. None while one runs or none waits.
    pub fn start_next(&mut self) -> Option<&mut Slot<T>> {
        if self.slots.iter().any(|s| s.phase == Phase::Running) {
            return None;
        }
        let s = self.slots.iter_mut().find(|s| s.phase == Phase::Queued)?;
        s.phase = Phase::Running;
        Some(s)
    }

    /// The running job has ended.
    pub fn finish(&mut self, id: u64) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.id == id && s.phase == Phase::Running) {
            s.phase = Phase::Finished;
        }
    }

    /// A waiting job taken off the queue (Cancel before it started): it is gone, never run. None if it is not waiting.
    pub fn cancel_queued(&mut self, id: u64) -> Option<T> {
        let i = self.slots.iter().position(|s| s.id == id && s.phase == Phase::Queued)?;
        Some(self.slots.remove(i).job)
    }

    /// A finished job taken off the list. False if it is not finished (a running or waiting one is never dropped here).
    pub fn dismiss(&mut self, id: u64) -> bool {
        match self.slots.iter().position(|s| s.id == id && s.phase == Phase::Finished) {
            Some(i) => {
                self.slots.remove(i);
                true
            }
            None => false,
        }
    }

    /// Every finished job taken off the list.
    pub fn dismiss_finished(&mut self) {
        self.slots.retain(|s| s.phase != Phase::Finished);
    }

    pub fn running(&self) -> Option<&Slot<T>> {
        self.slots.iter().find(|s| s.phase == Phase::Running)
    }

    pub fn running_mut(&mut self) -> Option<&mut Slot<T>> {
        self.slots.iter_mut().find(|s| s.phase == Phase::Running)
    }

    pub fn get(&self, id: u64) -> Option<&Slot<T>> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// How many wait.
    pub fn queued(&self) -> usize {
        self.slots.iter().filter(|s| s.phase == Phase::Queued).count()
    }

    /// Whether something runs or waits.
    pub fn busy(&self) -> bool {
        self.slots.iter().any(|s| s.phase != Phase::Finished)
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// In the order the panel lists them: the running one, the waiting ones (first to run first), then the finished ones
    /// (the last asked for first).
    pub fn listed(&self) -> impl Iterator<Item = &Slot<T>> {
        let by = |p: Phase| self.slots.iter().filter(move |s| s.phase == p);
        by(Phase::Running).chain(by(Phase::Queued)).chain(self.slots.iter().rev().filter(|s| s.phase == Phase::Finished))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Slot<T>> {
        self.slots.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_wait_their_turn_and_run_one_at_a_time() {
        let mut q = Queue::default();
        let a = q.push("a");
        let b = q.push("b");
        let c = q.push("c");
        assert_eq!(q.start_next().map(|s| s.job), Some("a"));
        assert!(q.start_next().is_none(), "one at a time");
        assert_eq!(q.running().map(|s| s.id), Some(a));
        assert_eq!(q.queued(), 2);
        // a waiting job can be cancelled; it never runs
        assert_eq!(q.cancel_queued(b), Some("b"));
        assert_eq!(q.cancel_queued(a), None, "the running one is stopped, not taken off");
        q.finish(a);
        assert_eq!(q.get(a).map(|s| s.phase), Some(Phase::Finished));
        assert_eq!(q.start_next().map(|s| s.id), Some(c));
        assert!(q.busy());
        q.finish(c);
        assert!(!q.busy());
        assert!(q.start_next().is_none());
        assert_eq!(q.len(), 2, "finished jobs stay listed until dismissed");
    }

    #[test]
    fn listed_running_then_waiting_then_finished_newest_first() {
        let mut q = Queue::default();
        let ids: Vec<u64> = (0..5).map(|i| q.push(i)).collect();
        q.start_next();
        q.finish(ids[0]);
        q.start_next();
        q.finish(ids[1]);
        q.start_next(); // ids[2] runs
        let order: Vec<i32> = q.listed().map(|s| s.job).collect();
        assert_eq!(order, vec![2, 3, 4, 1, 0]);
    }

    #[test]
    fn only_finished_jobs_are_dismissed() {
        let mut q = Queue::default();
        let a = q.push(1);
        let b = q.push(2);
        q.start_next();
        assert!(!q.dismiss(a), "running");
        assert!(!q.dismiss(b), "waiting");
        q.finish(a);
        assert!(q.dismiss(a));
        assert_eq!(q.len(), 1);
        q.start_next();
        q.finish(b);
        let c = q.push(3);
        q.dismiss_finished();
        assert_eq!(q.iter().map(|s| s.id).collect::<Vec<_>>(), vec![c]);
        assert!(!q.is_empty());
    }
}
