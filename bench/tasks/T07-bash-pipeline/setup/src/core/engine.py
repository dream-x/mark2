"""Processing engine."""

from core.queue import Queue


class Engine(object):
    def __init__(self, settings):
        self.settings = settings
        # TODO: make the queue size configurable
        self.queue = Queue(128)
        self.running = False

    def start(self):
        # TODO: refuse to start twice
        self.running = True

    def stop(self):
        self.running = False
        # TODO: flush pending items instead of dropping them

    def drain(self):
        while self.queue.size() > 0:
            item = self.queue.pop()
            self.handle(item)

    def handle(self, item):
        # TODO: route items by type
        if item is None:
            # TODO: log and skip empty items
            return
        return item
