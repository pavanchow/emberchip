//! A mutex with priority inheritance.
//!
//! The mutex itself tracks its owner and the queue of tasks blocked on it. The
//! inheritance decision (boosting the owner so it cannot be starved by medium
//! priority tasks) is computed by the kernel, which can see every task, but the
//! raw state it reasons over lives here.

#[derive(Clone, Debug, Default)]
pub struct Mutex {
    pub owner: Option<usize>,
    pub waiters: Vec<usize>,
}

impl Mutex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_free(&self) -> bool {
        self.owner.is_none()
    }

    /// Try to take the mutex for `task`. Succeeds only if it is free.
    pub fn try_acquire(&mut self, task: usize) -> bool {
        if self.owner.is_none() {
            self.owner = Some(task);
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

    /// Release the mutex. Returns the next owner if a waiter was granted it.
    /// The highest-priority waiter is chosen, using `priority_of` to rank them,
    /// with the lowest task id breaking ties for determinism.
    pub fn release(&mut self, task: usize, priority_of: impl Fn(usize) -> u8) -> Option<usize> {
        if self.owner != Some(task) {
            return None;
        }
        self.owner = None;
        if self.waiters.is_empty() {
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
        let next = self.waiters.remove(best);
        self.owner = Some(next);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::Mutex;

    #[test]
    fn mutual_exclusion() {
        let mut m = Mutex::new();
        assert!(m.try_acquire(1));
        assert!(!m.try_acquire(2));
        assert!(!m.is_free());
    }

    #[test]
    fn release_grants_highest_priority_waiter() {
        let mut m = Mutex::new();
        m.try_acquire(0);
        m.add_waiter(1);
        m.add_waiter(2);
        // task 2 has higher priority than task 1
        let prio = |t: usize| if t == 2 { 9 } else { 3 };
        assert_eq!(m.release(0, prio), Some(2));
        assert_eq!(m.owner, Some(2));
    }

    #[test]
    fn release_with_no_waiters_frees() {
        let mut m = Mutex::new();
        m.try_acquire(0);
        assert_eq!(m.release(0, |_| 0), None);
        assert!(m.is_free());
    }
}
