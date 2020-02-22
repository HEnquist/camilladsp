use std::default::Default;

pub struct FifoQueue<T> {
    index_oldest: usize,
    length: usize,
    capacity: usize,
    data: Vec<T>,
}

impl<T: Clone + Default> FifoQueue<T> {
    #[allow(dead_code)]
    pub fn new(capacity: usize) -> FifoQueue<T> {
        let data: Vec<T> = vec![Default::default(); capacity];
        let index_oldest = 0;
        FifoQueue {
            index_oldest,
            length: 0,
            capacity,
            data,
        }
    }

    pub fn filled_with(capacity: usize, value: T) -> FifoQueue<T> {
        let data: Vec<T> = vec![value; capacity];
        let index_oldest = 0;
        FifoQueue {
            index_oldest,
            length: capacity,
            capacity,
            data,
        }
    }

    pub fn push(&mut self, value: T) -> Result<(), &str> {
        if self.length == self.capacity {
            Err("The queue is full")
        } else {
            let mut new_index = self.index_oldest + self.length;
            if new_index >= self.capacity {
                new_index -= self.capacity;
            }
            self.data[new_index] = value;
            self.length += 1;
            Ok(())
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.length > 0 {
            let value = self.data[self.index_oldest].clone();
            self.index_oldest += 1;
            if self.index_oldest == self.capacity {
                self.index_oldest = 0;
            }
            self.length -= 1;
            Some(value)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn length(&self) -> usize {
        self.length
    }

    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use fifoqueue::FifoQueue;

    #[test]
    fn make_empty() {
        let q: FifoQueue<usize> = FifoQueue::new(5);
        assert_eq!(q.length(), 0);
        assert_eq!(q.capacity(), 5);
    }

    #[test]
    fn add_few() {
        let mut q: FifoQueue<usize> = FifoQueue::new(5);
        q.push(1).unwrap();
        q.push(2).unwrap();
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), None);
        assert_eq!(q.length(), 0);
    }
    #[test]
    fn wrap_around() {
        let mut q: FifoQueue<usize> = FifoQueue::new(3);
        q.push(1).unwrap();
        q.push(2).unwrap();
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), None);
        q.push(3).unwrap();
        q.push(4).unwrap();
        q.push(5).unwrap();
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), Some(5));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn overfill() {
        let mut q: FifoQueue<usize> = FifoQueue::new(3);
        assert_eq!(q.push(1), Ok(()));
        assert_eq!(q.push(2), Ok(()));
        assert_eq!(q.push(3), Ok(()));
        assert_eq!(q.push(4), Err("The queue is full"));
    }

    #[test]
    fn prefilled() {
        let mut q: FifoQueue<usize> = FifoQueue::filled_with(3, 1);
        assert_eq!(q.push(4), Err("The queue is full"));
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), None);
    }
}
