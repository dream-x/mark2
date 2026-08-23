"""Demo entry point. Run with: python3 src/main.py"""

from orders import checkout_total, estimate_total

DEMO_SKU = "B-22"
DEMO_QTY = 2
DEMO_TIER = "silver"


def main():
    print("sku: {0} qty: {1} tier: {2}".format(DEMO_SKU, DEMO_QTY, DEMO_TIER))
    print("estimate: {0}".format(estimate_total(DEMO_SKU, DEMO_QTY, DEMO_TIER)))
    print("charged:  {0}".format(checkout_total(DEMO_SKU, DEMO_QTY, DEMO_TIER)))


if __name__ == "__main__":
    main()
