# Architecture notes

- Every domain rule lives in exactly one module; the API layer
  (`ledgerly.api.handlers`) only translates HTTP to function calls.
- Rejections are `LedgerlyError` subclasses with a stable code `E-NNNN`.
  `ledgerly.api.errors` maps codes to HTTP statuses,
  `ledgerly.notifications.templates` maps them to customer texts.
- Money is integer cents; rates are basis points. No floats in money paths.
- `ledgerly.legacy` holds v1 code that batch jobs still replay. It is not
  imported by the live code and must keep its historical behaviour.
- Marketplace orders are refunded by the seller under the seller agreement
  (`ledgerly.marketplace.seller_refunds`); the shop's own refunds are
  `ledgerly.payments.refunds`.
