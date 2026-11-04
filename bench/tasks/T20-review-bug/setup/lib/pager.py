"""Paging helpers for the ledger tools.

Pages are numbered from 1. The helpers only compute indexes into a sequence
and never touch the data, so the same arithmetic serves lists, files and
database queries.
"""

DEFAULT_PER_PAGE = 20


class PageError(ValueError):
    """Raised for a page number or a page size that makes no sense."""


def page_count(total, per_page=DEFAULT_PER_PAGE):
    """Return how many pages ``total`` items fill, at least 1.

    An empty ledger still has one (empty) page, so "page 1 of 1" is always
    a valid thing to print.
    """
    if per_page < 1:
        raise PageError("per_page must be positive, got %r" % (per_page,))
    if total <= 0:
        return 1
    return (total + per_page - 1) // per_page


def check_page(page, total, per_page=DEFAULT_PER_PAGE):
    """Raise PageError unless ``page`` exists for ``total`` items."""
    count = page_count(total, per_page)
    if page < 1 or page > count:
        raise PageError("page %d is out of range 1..%d" % (page, count))


def page_bounds(page, total, per_page=DEFAULT_PER_PAGE):
    """Return ``(start, stop)``, the half-open index range of the items on
    ``page``, ready for slicing: ``items[start:stop]``.

    The only page of an empty ledger is empty: ``start == stop``.
    """
    check_page(page, total, per_page)
    start = (page - 1) * per_page
    stop = min(start + per_page, total)
    return start, stop


def paginate(items, page, per_page=DEFAULT_PER_PAGE):
    """Return the items shown on ``page``."""
    start, stop = page_bounds(page, len(items), per_page)
    return items[start:stop]


def clamp_page(page, total, per_page=DEFAULT_PER_PAGE):
    """Return the existing page nearest to ``page``.

    Interactive tools use it so that "page 99" of a short ledger shows the
    last page instead of an error.
    """
    return max(1, min(page, page_count(total, per_page)))


def page_label(page, total, per_page=DEFAULT_PER_PAGE):
    """Human readable position such as ``"page 2 of 5"``."""
    check_page(page, total, per_page)
    return "page %d of %d" % (page, page_count(total, per_page))
