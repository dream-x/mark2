"""A very small FIFO queue."""


class Queue(object):
    def __init__(self, capacity):
        self.capacity = capacity
        self.items = []

    def push(self, item):
        # TODO: raise when the queue is full
        self.items.append(item)

    def pop(self):
        # TODO: return None instead of raising on an empty queue
        return self.items.pop(0)

    def size(self):
        return len(self.items)

    def clear(self):
        # TODO: notify waiters after clearing
        self.items = []
