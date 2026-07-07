const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;

pub struct ScrollbackBuffer<T: AsRef<[u8]>> {
    buffer: Vec<T>,
    bytes: usize,
    max_bytes: usize,
}

impl<T: AsRef<[u8]>> Default for ScrollbackBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: AsRef<[u8]>> ScrollbackBuffer<T> {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            bytes: 0,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    pub fn push(&mut self, item: T) {
        let len = item.as_ref().len();
        self.buffer.push(item);
        self.bytes += len;
        while self.bytes > self.max_bytes && !self.buffer.is_empty() {
            let removed = self.buffer.remove(0);
            self.bytes -= removed.as_ref().len();
        }
    }

    pub fn items(&self) -> &[T] {
        &self.buffer
    }
}
