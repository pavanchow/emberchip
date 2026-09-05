//! A periodic hardware timer. It counts down from a reload value and raises an
//! interrupt when it wraps. With `reload == 1` it fires every clock, which is
//! the system tick that drives the scheduler.

#[derive(Clone, Debug)]
pub struct Timer {
    reload: u64,
    counter: u64,
    fired: u64,
}

impl Timer {
    pub fn new(reload: u64) -> Self {
        let reload = reload.max(1);
        Self {
            reload,
            counter: reload,
            fired: 0,
        }
    }

    /// Advance one clock. Returns `true` on the clock where the timer wraps and
    /// raises its interrupt.
    pub fn clock(&mut self) -> bool {
        self.counter -= 1;
        if self.counter == 0 {
            self.counter = self.reload;
            self.fired += 1;
            true
        } else {
            false
        }
    }

    pub fn interrupts_raised(&self) -> u64 {
        self.fired
    }

    pub fn reload(&self) -> u64 {
        self.reload
    }
}

#[cfg(test)]
mod tests {
    use super::Timer;

    #[test]
    fn fires_every_clock_when_reload_one() {
        let mut t = Timer::new(1);
        for _ in 0..5 {
            assert!(t.clock());
        }
        assert_eq!(t.interrupts_raised(), 5);
    }

    #[test]
    fn fires_on_wrap() {
        let mut t = Timer::new(3);
        assert!(!t.clock());
        assert!(!t.clock());
        assert!(t.clock());
        assert!(!t.clock());
        assert_eq!(t.interrupts_raised(), 1);
    }

    #[test]
    fn zero_reload_is_clamped() {
        let mut t = Timer::new(0);
        assert_eq!(t.reload(), 1);
        assert!(t.clock());
    }
}
