//! A bounded message queue. Sends block when full, receives block when empty.
//! Delivery is strict FIFO: values come out in the order they went in, with no
//! loss and no duplication.

use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Queue {
    buf: VecDeque<u32>,
    capacity: usize,
    pub recv_waiters: Vec<usize>,
    pub send_waiters: Vec<usize>,
    pub sent: u64,
    pub received: u64,
}

impl Queue {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            capacity: capacity.max(1),
            recv_waiters: Vec::new(),
            send_waiters: Vec::new(),
            sent: 0,
            received: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.buf.len() >= self.capacity
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Push a value if there is room. Returns `false` if the queue is full.
    pub fn try_send(&mut self, value: u32) -> bool {
        if self.is_full() {
            return false;
        }
        self.buf.push_back(value);
        self.sent += 1;
        true
    }

    /// Pop the oldest value, or `None` if empty.
    pub fn try_recv(&mut self) -> Option<u32> {
        let v = self.buf.pop_front();
        if v.is_some() {
            self.received += 1;
        }
        v
    }

    fn pop_best(list: &mut Vec<usize>, priority_of: &impl Fn(usize) -> u8) -> Option<usize> {
        if list.is_empty() {
            return None;
        }
        let mut best = 0usize;
        for (i, &w) in list.iter().enumerate() {
            let cur = list[best];
            if priority_of(w) > priority_of(cur) || (priority_of(w) == priority_of(cur) && w < cur) {
                best = i;
            }
        }
        Some(list.remove(best))
    }

    pub fn wake_receiver(&mut self, priority_of: impl Fn(usize) -> u8) -> Option<usize> {
        Self::pop_best(&mut self.recv_waiters, &priority_of)
    }

    pub fn wake_sender(&mut self, priority_of: impl Fn(usize) -> u8) -> Option<usize> {
        Self::pop_best(&mut self.send_waiters, &priority_of)
    }
}

#[cfg(test)]
mod tests {
    use super::Queue;

    #[test]
    fn fifo_order() {
        let mut q = Queue::new(4);
        assert!(q.try_send(10));
        assert!(q.try_send(20));
        assert!(q.try_send(30));
        assert_eq!(q.try_recv(), Some(10));
        assert_eq!(q.try_recv(), Some(20));
        assert_eq!(q.try_recv(), Some(30));
        assert_eq!(q.try_recv(), None);
    }

    #[test]
    fn full_blocks_send() {
        let mut q = Queue::new(2);
        assert!(q.try_send(1));
        assert!(q.try_send(2));
        assert!(!q.try_send(3));
        assert!(q.is_full());
    }

    #[test]
    fn no_loss_no_duplication() {
        let mut q = Queue::new(3);
        for i in 0..3 {
            assert!(q.try_send(i));
        }
        let mut seen = Vec::new();
        while let Some(v) = q.try_recv() {
            seen.push(v);
        }
        assert_eq!(seen, vec![0, 1, 2]);
        assert_eq!(q.sent, 3);
        assert_eq!(q.received, 3);
    }
}
