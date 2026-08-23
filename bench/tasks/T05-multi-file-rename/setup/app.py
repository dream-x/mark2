"""Storefront report. Run with: python3 app.py"""

from lib import average_item_price, calc_total, item_count, total_with_tax

CART = [
    {"sku": "A-11", "price": 20.0, "qty": 3},
    {"sku": "B-22", "price": 12.5, "qty": 2},
    {"sku": "C-33", "price": 7.25, "qty": 4},
]

EMPTY_CART = []


def report(cart):
    """Print a cart summary built on top of calc_total."""
    print("units: {0}".format(item_count(cart)))
    print("subtotal: {0:.2f}".format(calc_total(cart)))
    print("with tax: {0:.2f}".format(total_with_tax(cart)))
    print("avg unit: {0:.2f}".format(average_item_price(cart)))


def main():
    report(CART)
    print("empty subtotal: {0:.2f}".format(calc_total(EMPTY_CART)))


if __name__ == "__main__":
    main()
