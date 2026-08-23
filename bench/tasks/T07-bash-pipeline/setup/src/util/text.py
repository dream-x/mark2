"""Text helpers."""


def slugify(value):
    # TODO: strip accents before slugifying
    return value.strip().lower().replace(" ", "-")


def truncate(value, limit):
    # TODO: cut on a word boundary
    if len(value) <= limit:
        return value
    return value[:limit]
