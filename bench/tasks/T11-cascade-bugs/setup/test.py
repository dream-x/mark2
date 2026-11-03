"""Test suite for textkit.py. Run with: python3 test.py"""

import textkit


def check(label, got, want):
    if got != want:
        raise AssertionError("%s: got %r, expected %r" % (label, got, want))


def test_normalize_spaces():
    check("normalize_spaces/plain", textkit.normalize_spaces("a b"), "a b")
    check(
        "normalize_spaces/runs",
        textkit.normalize_spaces("  a\t b\n\nc  "),
        "a b c",
    )
    check("normalize_spaces/blank", textkit.normalize_spaces(" \t\n"), "")


def test_truncate_words():
    check(
        "truncate_words/short",
        textkit.truncate_words("one two", 5),
        "one two",
    )
    check(
        "truncate_words/exact-limit",
        textkit.truncate_words("one two three", 3),
        "one two three",
    )
    check(
        "truncate_words/cut",
        textkit.truncate_words("one two three four", 2),
        "one two...",
    )
    check(
        "truncate_words/limit-1",
        textkit.truncate_words("alpha beta", 1),
        "alpha...",
    )
    check(
        "truncate_words/messy-spaces",
        textkit.truncate_words("  a   b  c ", 2),
        "a b...",
    )


def test_wrap_line():
    check("wrap_line/empty", textkit.wrap_line("   ", 10), [])
    check(
        "wrap_line/fits",
        textkit.wrap_line("a bb ccc", 20),
        ["a bb ccc"],
    )
    check(
        "wrap_line/exact-width",
        textkit.wrap_line("aaa bbb ccc", 7),
        ["aaa bbb", "ccc"],
    )
    check(
        "wrap_line/greedy",
        textkit.wrap_line("the quick brown fox jumps", 10),
        ["the quick", "brown fox", "jumps"],
    )
    check(
        "wrap_line/long-word",
        textkit.wrap_line("hi extraordinarily ok", 5),
        ["hi", "extraordinarily", "ok"],
    )


def test_count_sentences():
    check("count_sentences/two", textkit.count_sentences("Hello. World!"), 2)
    check(
        "count_sentences/runs",
        textkit.count_sentences("Wait... what?! No."),
        3,
    )
    check(
        "count_sentences/no-terminator",
        textkit.count_sentences("one more thing"),
        1,
    )
    check("count_sentences/empty", textkit.count_sentences(""), 0)
    check("count_sentences/only-marks", textkit.count_sentences("?!"), 0)
    check(
        "count_sentences/leading-dots",
        textkit.count_sentences("...and then."),
        1,
    )


def test_slugify():
    check(
        "slugify/basic",
        textkit.slugify("  Hello, World!  "),
        "hello-world",
    )
    check("slugify/digits", textkit.slugify("Report 2026 Q3"), "report-2026-q3")
    check("slugify/symbols", textkit.slugify("--a__b--"), "a-b")


def main():
    for name, test in [
        ("normalize_spaces", test_normalize_spaces),
        ("truncate_words", test_truncate_words),
        ("wrap_line", test_wrap_line),
        ("count_sentences", test_count_sentences),
        ("slugify", test_slugify),
    ]:
        test()
        print("%s: ok" % name)
    print("all tests passed")


if __name__ == "__main__":
    main()
