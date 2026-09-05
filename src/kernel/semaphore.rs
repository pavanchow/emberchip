//! A counting semaphore. `wait` takes a unit or blocks, `signal` returns a unit
//! and wakes the highest-priority waiter. Signals are never lost or duplicated:
//! every signal either satisfies a waiter or increments the count exactly once.

#[derive(Clone, Debug)]
pub struct Semaphore {
    count: u32,
    max: u32,
    pub waiters: Vec<usize>,
}

impl Semaphore {
    pub fn new(initial: u32, max: u32) -> Self {
        Self {
            count: initial.min(max),
            max,
            waiters: Vec::new(),
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    /// Try to take a unit. Returns `true` if it succeeded without blocking.
    pub fn try_wait(&mut self) -> bool {
        if self.count > 0 {
            self.count -= 1;
            true
        } else {
            false
        }
    }

    pub fn add_waiter(&mut self, task: usize) {
        if !self.waiters.contains(&task) {
            self.waiters.push(task);
        }
    }

    /// Signal the semaphore. If a task is waiting, wake the highest-priority one
    /// and hand the unit straight to it (return its id). Otherwise increment the
    /// count, saturating at `max`.
    pub fn signal(&mut self, priority_of: impl Fn(usize) -> u8) -> Option<usize> {
        if self.waiters.is_empty() {
            if self.count < self.max {
                self.count += 1;
            }
            return None;
        }
        let mut best = 0usize;
        for (i, &w) in self.waiters.iter().enumerate() {
            let cur = self.waiters[best];
            if priority_of(w) > priority_of(cur)
                || (priority_of(w) == priority_of(cur) && w < cur)
            {
                best = i;
            }
        }
        Some(self.waiters.remove(best))
    }
}

#[cfg(test)]
mod tests {
    use super::Semaphore;

    #[test]
    fn wait_consumes_count() {
        let mut s = Semaphore::new(2, 4);
        assert!(s.try_wait());
        assert!(s.try_wait());
        assert!(!s.try_wait());
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn signal_without_waiters_increments() {
        let mut s = Semaphore::new(0, 4);
        assert_eq!(s.signal(|_| 0), None);
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn signal_wakes_highest_priority_waiter() {
        let mut s = Semaphore::new(0, 4);
        s.add_waiter(1);
        s.add_waiter(2);
        let prio = |t: usize| if t == 2 { 9 } else { 1 };
        assert_eq!(s.signal(prio), Some(2));
        // count stays zero, the unit went straight to the woken task
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn count_saturates_at_max() {
        let mut s = Semaphore::new(0, 2);
        s.signal(|_| 0);
        s.signal(|_| 0);
        s.signal(|_| 0);
        assert_eq!(s.count(), 2);
    }
}
