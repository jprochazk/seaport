/// FIFO queue backed by a ring buffer
#[derive(Debug, Clone)]
pub struct Queue<T> {
  buffer: Vec<Option<T>>,
  head: usize,
  tail: usize,
}

impl<T> Queue<T> {
  pub fn new(capacity: usize) -> Self {
    let mut buffer = Vec::new();
    buffer.resize_with(capacity, Default::default);
    Self {
      buffer,
      head: 0,
      tail: 0,
    }
  }

  pub fn remove<F: Fn(&T) -> bool>(&mut self, predicate: F) -> Option<T> {
    match self
      .iter()
      .enumerate()
      .find(|(n, (i, v))| predicate(*v))
      .map(|(n, (i, _))| (i, n))
    {
      Some((i, n)) => {
        // Safety: The index exists because the above `find` produced it.
        let item = unsafe { self.buffer[i].take().unwrap_unchecked() };
        self.head = if self.head == 0 {
          self.buffer.len() - 1
        } else {
          self.head - 1
        };

        let mut a = i;
        let mut b = (i + 1) % self.buffer.len();
        let mut c = self.remaining() - n;
        while c > 0 {
          self.buffer.swap(a, b);
          a = (a + 1) % self.buffer.len();
          b = (b + 1) % self.buffer.len();
          c -= 1;
        }

        Some(item)
      }
      None => None,
    }
  }

  pub fn remaining(&self) -> usize {
    use std::cmp::Ordering::*;
    let (tail, head) = (self.tail, self.head);
    match head.cmp(&tail) {
      Greater => head - tail,
      Equal => {
        if self.buffer[self.tail].is_some() {
          self.buffer.len()
        } else {
          0
        }
      }
      Less => self.buffer.len() - tail + head,
    }
  }

  pub fn is_empty(&self) -> bool {
    self.remaining() == 0
  }

  /// Returns `Some(T)` if the queue is full
  pub fn put(&mut self, item: T) -> Option<T> {
    if self.buffer[self.head].is_none() {
      self.buffer[self.head] = Some(item);
      self.head = (self.head + 1) % self.buffer.len();
      None
    } else {
      Some(item)
    }
  }

  pub fn get(&mut self) -> Option<T> {
    let item = self.buffer[self.tail].take();
    if item.is_some() {
      self.tail = (self.tail + 1) % self.buffer.len();
    }
    item
  }

  pub fn peek(&self) -> Option<&T> {
    self.buffer[self.tail].as_ref()
  }

  pub fn iter(&self) -> Iter<'_, T> {
    Iter {
      queue: self,
      index: self.tail,
    }
  }

  pub fn drain(&mut self) -> Drain<'_, T> {
    Drain(self)
  }
}

pub struct Iter<'a, T> {
  queue: &'a Queue<T>,
  index: usize,
}

impl<'a, T> Iterator for Iter<'a, T> {
  type Item = (usize, &'a T);

  fn next(&mut self) -> Option<Self::Item> {
    if let Some(item) = self.queue.buffer[self.index].as_ref() {
      let index = self.index;
      self.index = (self.index + 1) % self.queue.buffer.len();
      Some((index, item))
    } else {
      None
    }
  }
}

pub struct Drain<'a, T>(&'a mut Queue<T>);

impl<'a, T> Iterator for Drain<'a, T> {
  type Item = T;

  fn next(&mut self) -> Option<Self::Item> {
    self.0.get()
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    (self.0.remaining(), Some(self.0.remaining()))
  }
}

/// Queue that maintains two inner FIFO queues, `read` and `write`,
/// with a method to swap them.
///
/// This can be used to avoid infinite loops when re-inserting items
/// into the same queue right after retrieving them.
#[derive(Debug, Clone)]
pub struct SwapQueue<T> {
  read: Queue<T>,
  write: Queue<T>,
}

impl<T> SwapQueue<T> {
  pub fn new(capacity: usize) -> Self {
    Self {
      read: Queue::new(capacity),
      write: Queue::new(capacity),
    }
  }

  pub fn remaining(&self) -> usize {
    self.read.remaining()
  }

  pub fn is_empty(&self) -> bool {
    self.read.is_empty()
  }

  pub fn put(&mut self, item: T) -> Option<T> {
    self.write.put(item)
  }

  pub fn get(&mut self) -> Option<T> {
    self.read.get()
  }

  pub fn remove<F: Fn(&T) -> bool>(&mut self, predicate: F) -> Option<T> {
    self
      .read
      .remove(&predicate)
      .or_else(|| self.write.remove(&predicate))
  }

  pub fn swap(&mut self) {
    std::mem::swap(&mut self.read, &mut self.write);
  }

  pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
    self.read.drain()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn put_and_get() {
    let mut queue = Queue::new(4);

    for i in 0..4 {
      queue.put(i);
    }
    assert_eq!(queue.remaining(), 4);
    // don't accept more than `capacity`
    assert_eq!(queue.put(4), Some(4));
    assert_eq!(queue.remaining(), 4);
    let items = queue.drain().collect::<Vec<i32>>();
    assert_eq!(queue.remaining(), 0);
    assert_eq!(&items[..], &[0, 1, 2, 3]);
  }

  #[test]
  fn queue_wraps() {
    let mut queue = Queue::new(4);

    for i in 0..4 {
      queue.put(i);
    }
    assert_eq!(queue.remaining(), 4);
    assert_eq!(queue.get(), Some(0));
    assert_eq!(queue.remaining(), 3);
    assert_eq!(queue.put(4), None);
    assert_eq!(queue.remaining(), 4);
    // don't accept more than `capacity`
    assert_eq!(queue.put(5), Some(5));
    assert_eq!(queue.remaining(), 4);
    // queue drains correctly starting at `tail` and wrapping around
    assert_eq!(&queue.buffer[..], &[Some(4), Some(1), Some(2), Some(3)]);
    let items = queue.drain().collect::<Vec<i32>>();
    assert_eq!(queue.remaining(), 0);
    assert_eq!(&items[..], &[1, 2, 3, 4]);
  }

  #[test]
  fn swap_queue() {
    let mut queue = SwapQueue::new(4);

    for i in 0..4 {
      queue.put(i);
    }
    // queue still has the capacity we expect
    assert_eq!(queue.put(4), Some(4));
    for _ in 0..queue.remaining() {
      let item = queue.get().unwrap();
      queue.put(item);
    }
    assert_eq!(queue.remaining(), 0);
    assert_eq!(queue.get(), None);
    queue.swap();
    assert_eq!(queue.remaining(), 4);
    let items = queue.drain().collect::<Vec<_>>();
    assert_eq!(queue.remaining(), 0);
    assert_eq!(&items[..], &[0, 1, 2, 3]);
  }

  #[test]
  fn remove_from_queue() {
    // remove from end
    let mut queue = Queue::new(4);
    for i in 0..4 {
      queue.put(i);
    }
    assert_eq!(&queue.buffer[..], &[Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!(queue.remove(|v| *v == 3), Some(3));
    assert_eq!(&queue.buffer[..], &[Some(0), Some(1), Some(2), None]);
    assert_eq!(queue.tail, 0);
    assert_eq!(queue.head, 3);

    // remove followed by wrapping left shift
    let mut queue = Queue::new(4);
    for i in 0..4 {
      queue.put(i);
    }
    queue.get();
    queue.put(4);
    assert_eq!(&queue.buffer[..], &[Some(4), Some(1), Some(2), Some(3)]);
    assert_eq!(queue.remove(|v| *v == 2), Some(2));
    assert_eq!(&queue.buffer[..], &[None, Some(1), Some(3), Some(4)]);
    assert_eq!(queue.tail, 1);
    assert_eq!(queue.head, 0);

    // remove from start
    let mut queue = Queue::new(4);
    for i in 0..4 {
      queue.put(i);
    }
    assert_eq!(&queue.buffer[..], &[Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!(queue.remove(|v| *v == 0), Some(0));
    assert_eq!(&queue.buffer[..], &[Some(1), Some(2), Some(3), None]);
    assert_eq!(queue.tail, 0);
    assert_eq!(queue.head, 3);
  }

  #[test]
  fn remove_from_swap_queue() {
    // item is in `read`
    let mut queue = SwapQueue::new(4);
    for i in 0..4 {
      queue.put(i);
    }
    queue.swap();
    assert_eq!(
      &queue.read.buffer[..],
      &[Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(&queue.write.buffer[..], &[None, None, None, None]);
    assert_eq!(queue.remove(|v| *v == 0), Some(0));
    assert_eq!(&queue.read.buffer[..], &[Some(1), Some(2), Some(3), None]);
    assert_eq!(&queue.write.buffer[..], &[None, None, None, None]);
    for item in queue.drain().collect::<Vec<_>>().into_iter() {
      queue.put(item);
    }
    // next item is in `write`
    assert_eq!(&queue.read.buffer[..], &[None, None, None, None]);
    assert_eq!(&queue.write.buffer[..], &[Some(1), Some(2), Some(3), None]);
    assert_eq!(queue.remove(|v| *v == 2), Some(2));
    assert_eq!(&queue.read.buffer[..], &[None, None, None, None]);
    assert_eq!(&queue.write.buffer[..], &[Some(1), Some(3), None, None]);
  }
}
