"""Plain-text helpers used by the report printer."""


def normalize_spaces(text):
    """Collapse every run of whitespace in `text` into a single space.

    Leading and trailing whitespace is removed. Tabs and newlines count as
    whitespace too, so "  a\\t b\\n" becomes "a b".
    """
    return " ".join(text.split())


def truncate_words(text, limit):
    """Shorten `text` to at most `limit` words.

    Words are separated by any whitespace and are joined back with single
    spaces. When `text` has more than `limit` words, exactly the first
    `limit` words are kept and "..." is appended directly to the last kept
    word: truncate_words("one two three", 2) == "one two...". When `text`
    has `limit` words or fewer, all of them are returned and no "..." is
    added. `limit` is always at least 1.
    """
    words = text.split()
    if len(words) <= limit:
        return " ".join(words)
    return " ".join(words[:limit - 1]) + "..."


def wrap_line(text, width):
    """Greedily wrap `text` into lines of at most `width` characters.

    Words are never split and are separated by single spaces inside a line.
    Each line takes as many words as fit, and a line that is exactly `width`
    characters long still fits. A word longer than `width` goes on a line
    of its own. Returns the list of lines; text without words gives [].
    """
    lines = []
    current = ""
    for word in text.split():
        if not current:
            current = word
        elif len(current) + 1 + len(word) < width:
            current = current + " " + word
        else:
            lines.append(current)
            current = word
    if current:
        lines.append(current)
    return lines


def count_sentences(text):
    """Return the number of sentences in `text`.

    A sentence is a stretch of text that contains at least one letter or
    digit and ends with a run of ".", "!" or "?" characters, so "Wait..."
    and "Really?!" are one sentence each. A final stretch with letters or
    digits but without a terminator still counts as a sentence. Terminators
    that follow no letter or digit neither start nor end a sentence: "" and
    "?!" contain 0 sentences, "...and then." contains 1.
    """
    count = 0
    in_sentence = True
    for char in text:
        if char in ".!?":
            if in_sentence:
                count += 1
                in_sentence = False
        elif char.isalnum():
            in_sentence = True
    if in_sentence:
        count += 1
    return count


def slugify(text):
    """Turn `text` into a lowercase URL slug.

    Every run of characters that are not ASCII letters or digits becomes a
    single "-", and the slug never starts or ends with "-":
    slugify("  Hello, World!  ") == "hello-world".
    """
    parts = []
    current = []
    for char in text.lower():
        if char.isascii() and char.isalnum():
            current.append(char)
        elif current:
            parts.append("".join(current))
            current = []
    if current:
        parts.append("".join(current))
    return "-".join(parts)
