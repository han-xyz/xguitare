/// The largest level among the last `window + 1` pushed, as a monotonic queue in a fixed ring.
pub(crate) struct WindowPeak {
    entries: Vec<(usize, f32)>,
    head: usize,
    len: usize,
    count: usize,
}

impl WindowPeak {
    pub(crate) fn new(capacity: usize) -> WindowPeak {
        WindowPeak {
            entries: vec![(0, 0.0); capacity.max(1)],
            head: 0,
            len: 0,
            count: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.count = 0;
    }

    fn slot(&self, offset: usize) -> usize {
        (self.head + offset) % self.entries.len()
    }

    pub(crate) fn push(&mut self, level: f32, window: usize) -> f32 {
        while self.len > 0 && self.entries[self.slot(self.len - 1)].1 <= level {
            self.len -= 1;
        }
        while self.len > 0 && self.entries[self.head].0 + window < self.count {
            self.head = self.slot(1);
            self.len -= 1;
        }
        let tail = self.slot(self.len);
        self.entries[tail] = (self.count, level);
        self.len += 1;
        self.count = self.count.wrapping_add(1);
        self.entries[self.head].1
    }
}
