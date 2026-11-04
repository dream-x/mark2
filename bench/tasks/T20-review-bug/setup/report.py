"""Print a ledger report split into pages, with a subtotal on every page.

Usage: python3 report.py LEDGER.csv [PER_PAGE]

The ledger is a CSV file with ``date,payee,amount`` columns. Every page ends
with the subtotal of its own rows and the report ends with the grand total,
so the page subtotals always add up to the grand total.
"""
import csv
import sys

from lib.pager import DEFAULT_PER_PAGE, PageError, page_bounds, page_count

RULE = "-" * 48


def load_rows(path):
    """Read the ledger; ``amount`` becomes a float."""
    rows = []
    with open(path, newline="", encoding="utf-8") as handle:
        for record in csv.DictReader(handle):
            rows.append(
                {
                    "date": record["date"],
                    "payee": record["payee"],
                    "amount": float(record["amount"]),
                }
            )
    return rows


def format_row(row):
    return "%-10s  %-24s %10.2f" % (row["date"], row["payee"], row["amount"])


def render_page(rows, page, per_page=DEFAULT_PER_PAGE):
    """Lines of one report page: a header, the page's rows, their subtotal."""
    first, last = page_bounds(page, len(rows), per_page)
    pages = page_count(len(rows), per_page)
    lines = [
        "Rows %d-%d of %d, page %d/%d" % (first + 1, last + 1, len(rows), page, pages),
        RULE,
    ]
    subtotal = 0.0
    for row in rows[first:last + 1]:
        lines.append(format_row(row))
        subtotal += row["amount"]
    lines.append(RULE)
    lines.append("%-36s %10.2f" % ("page subtotal", subtotal))
    return lines


def render_report(rows, per_page=DEFAULT_PER_PAGE):
    """All pages of the report separated by blank lines, then the grand total."""
    if not rows:
        return ["The ledger is empty.", "%-36s %10.2f" % ("grand total", 0.0)]
    lines = []
    for page in range(1, page_count(len(rows), per_page) + 1):
        if page > 1:
            lines.append("")
        lines.extend(render_page(rows, page, per_page))
    lines.append("")
    lines.append("%-36s %10.2f" % ("grand total", sum(row["amount"] for row in rows)))
    return lines


def main(argv):
    if len(argv) not in (2, 3):
        print("usage: python3 report.py LEDGER.csv [PER_PAGE]", file=sys.stderr)
        return 1
    try:
        per_page = int(argv[2]) if len(argv) == 3 else DEFAULT_PER_PAGE
        page_count(0, per_page)  # raises PageError for a size below 1
    except (ValueError, PageError):
        print("report: PER_PAGE must be a positive integer", file=sys.stderr)
        return 1
    try:
        rows = load_rows(argv[1])
    except (OSError, KeyError, ValueError) as error:
        print("report: cannot read %s: %s" % (argv[1], error), file=sys.stderr)
        return 1
    print("\n".join(render_report(rows, per_page)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
