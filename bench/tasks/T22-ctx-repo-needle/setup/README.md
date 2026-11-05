# ledgerly

Backend of the shop: orders, payments, refunds, marketplace, shipping and
billing. Python 3.11+, no third-party dependencies.

    src/ledgerly/
      api/            HTTP handlers and error-code mapping
      billing/        invoices, credit notes, dunning, proration
      catalog/        products, prices, bundles
      customers/      accounts, loyalty
      inventory/      stock, reservations, transfers
      legacy/         v1 code kept for replay/archive jobs (not imported)
      marketplace/    sellers, payouts, disputes, seller-funded refunds
      notifications/  customer texts, email, scheduling
      payments/       capture, chargebacks, gateways, ledger, refunds
      reporting/      daily reports, reconciliation
      shipping/       labels, carriers, tracking, returns
      tax/            rates, VAT
      util/           shared helpers and the base error

Tests:

    python3 -m unittest discover -s tests

Error codes and their meaning: docs/errors.md.
