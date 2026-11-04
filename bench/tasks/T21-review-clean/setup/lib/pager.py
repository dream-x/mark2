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
    """Raise PageError unless ``page`` is one of the pages ``total`` items fill."""
    pages = page_count(total, per_page)
    if not 1 <= page <= pages:
        raise PageError("page %d is out of range 1..%d" % (page, pages))


def page_bounds(page, total, per_page=DEFAULT_PER_PAGE):
    """Return ``(first, last)``: the index of the first item on ``page`` and
    the index of its last item, both inclusive.

    The only page of an empty ledger is empty: ``last == first - 1``.
    """
    check_page(page, total, per_page)
    first = (page - 1) * per_page
    last = min(first + per_page, total) - 1
    return first, last


def paginate(items, page, per_page=DEFAULT_PER_PAGE):
    """Return the items shown on ``page``."""
    first, last = page_bounds(page, len(items), per_page)
    return items[first:last + 1]


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
