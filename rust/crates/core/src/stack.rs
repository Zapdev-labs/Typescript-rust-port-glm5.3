// Ported from tsc/internal/core/stack.go @ ec47d33c23e464a17cdf2475632cba629bee8763

pub struct Stack<T> {
    data: Vec<T>,
}

impl<T> Default for Stack<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Stack<T> {
    pub fn new() -> Stack<T> {
        Stack { data: Vec::new() }
    }

    pub fn push(&mut self, item: T) {
        self.data.push(item);
    }

    pub fn pop(&mut self) -> T {
        // PORT: Go zeroes the popped slot to help the GC; Rust's Vec::pop
        // moves the value out, which is the same observable behavior.
        self.data.pop().expect("stack is empty")
    }

    pub fn peek(&self) -> &T {
        self.data.last().expect("stack is empty")
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_push_pop_peek() {
        let mut s: Stack<i32> = Stack::new();
        assert_eq!(s.len(), 0);
        s.push(1);
        s.push(2);
        assert_eq!(s.len(), 2);
        assert_eq!(*s.peek(), 2);
        assert_eq!(s.pop(), 2);
        assert_eq!(s.pop(), 1);
        assert_eq!(s.len(), 0);
    }
}
