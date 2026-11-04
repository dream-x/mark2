"""Browse a ledger CSV page by page from the terminal.

Usage: python3 cli.py LEDGER.csv [--page N] [--per-page N] [--payee TEXT]

A page number past the end shows the last page, so "--page 999" is a quick
way to see the newest entries.
"""
import argparse
import csv
import sys

from lib.pager import DEFAULT_PER_PAGE, PageError, clamp_page, page_label, paginate


COLUMNS = ("date", "payee", "amount")


def read_ledger(path):
    """Rows of the ledger as dicts; amounts stay strings, they are only shown.

    Raises KeyError naming the columns the file lacks.
    """
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        missing = [name for name in COLUMNS if name not in (reader.fieldnames or [])]
        if missing:
            raise KeyError("missing columns: " + ", ".join(missing))
        return list(reader)


def matching(rows, payee):
    """Rows whose payee contains ``payee`` (case-insensitive); all if empty."""
    if not payee:
        return rows
    needle = payee.casefold()
    return [row for row in rows if needle in row["payee"].casefold()]


def build_parser():
    parser = argparse.ArgumentParser(description="Browse a ledger page by page.")
    parser.add_argument("ledger", help="CSV file with date,payee,amount columns")
    parser.add_argument("--page", type=int, default=1, help="page number, from 1")
    parser.add_argument(
        "--per-page", type=int, default=DEFAULT_PER_PAGE, help="rows per page"
    )
    parser.add_argument("--payee", default="", help="only rows whose payee has TEXT")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        rows = matching(read_ledger(args.ledger), args.payee)
        page = clamp_page(args.page, len(rows), args.per_page)
        shown = paginate(rows, page, args.per_page)
    except (OSError, KeyError) as error:
        print("cli: cannot read %s: %s" % (args.ledger, error), file=sys.stderr)
        return 1
    except PageError as error:
        print("cli: %s" % error, file=sys.stderr)
        return 2
    for row in shown:
        print("%-10s  %-24s %10s" % (row["date"], row["payee"], row["amount"]))
    print("-- %s, %d rows --" % (page_label(page, len(rows), args.per_page), len(rows)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
