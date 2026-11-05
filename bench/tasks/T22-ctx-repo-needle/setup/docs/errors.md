# Error codes

Every code the backend raises, the module that raises it and the HTTP
status the API answers with. Regenerated 2025-02-11.

| code | meaning | raised in | HTTP |
|---|---|---|---|
| E-1031 | unknown currency | `ledgerly.billing.invoices` | 400 |
| E-1043 | export row batch size too large | `ledgerly.legacy.exports_v1` | 422 |
| E-1110 | malformed seller reference | `ledgerly.marketplace.sellers` | 400 |
| E-1157 | payload max depth too large | `ledgerly.api.serializers` | 422 |
| E-1170 | tax line jurisdiction level too large | `ledgerly.tax.rates` | 422 |
| E-1183 | export row retries too large | `ledgerly.legacy.exports_v1` | 422 |
| E-1233 | invoice copies too large | `ledgerly.billing.invoices` | 422 |
| E-1263 | email size kb too large | `ledgerly.notifications.email` | 422 |
| E-1276 | unknown program | `ledgerly.customers.loyalty` | 400 |
| E-1496 | route rate per minute too large | `ledgerly.api.routes` | 422 |
| E-1499 | account fee: negative amount | `ledgerly.customers.accounts` | 422 |
| E-1556 | shipment reserve: negative amount | `ledgerly.shipping.carriers` | 422 |
| E-1584 | settlement amount cents too large | `ledgerly.reporting.reconciliation` | 422 |
| E-1586 | credit note revision too large | `ledgerly.billing.credit_notes` | 422 |
| E-1624 | loyalty entry multiplier bp too large | `ledgerly.customers.loyalty` | 422 |
| E-1626 | malformed reminder reference | `ledgerly.billing.dunning` | 400 |
| E-1788 | label copies too large | `ledgerly.shipping.labels` | 422 |
| E-1803 | legacy invoice year too large | `ledgerly.legacy.invoices_v1` | 422 |
| E-1844 | unknown schema name | `ledgerly.api.serializers` | 400 |
| E-1863 | unknown email | `ledgerly.customers.accounts` | 400 |
| E-1871 | reservation hold minutes too large | `ledgerly.inventory.reservations` | 422 |
| E-1996 | invoice reserve: negative amount | `ledgerly.billing.invoices` | 422 |
| E-2010 | dispute surcharge: negative amount | `ledgerly.marketplace.disputes` | 422 |
| E-2040 | malformed reservation reference | `ledgerly.inventory.reservations` | 400 |
| E-2048 | payout levy: negative amount | `ledgerly.marketplace.payouts` | 422 |
| E-2054 | settlement lag days too large | `ledgerly.reporting.reconciliation` | 422 |
| E-2062 | job max attempts too large | `ledgerly.notifications.scheduler` | 422 |
| E-2108 | unknown kind | `ledgerly.notifications.scheduler` | 400 |
| E-2244 | product weight g too large | `ledgerly.catalog.products` | 422 |
| E-2296 | malformed invoice reference | `ledgerly.billing.invoices` | 400 |
| E-2297 | gateway call surcharge: negative amount | `ledgerly.payments.gateways` | 422 |
| E-2315 | tax line rounding unit too large | `ledgerly.tax.rates` | 422 |
| E-2380 | proration days in period too large | `ledgerly.billing.proration` | 422 |
| E-2385 | account failed logins too large | `ledgerly.customers.accounts` | 422 |
| E-2393 | loyalty entry reserve: negative amount | `ledgerly.customers.loyalty` | 422 |
| E-2453 | malformed shipment reference | `ledgerly.shipping.carriers` | 400 |
| E-2503 | legacy invoice sequence too large | `ledgerly.legacy.invoices_v1` | 422 |
| E-2596 | malformed chargeback reference | `ledgerly.payments.chargebacks` | 400 |
| E-2598 | tracking event scan count too large | `ledgerly.shipping.tracking` | 422 |
| E-2625 | payload max items too large | `ledgerly.api.serializers` | 422 |
| E-2695 | proration amount cents too large | `ledgerly.billing.proration` | 422 |
| E-2756 | unknown template | `ledgerly.notifications.email` | 400 |
| E-2761 | transfer pallets too large | `ledgerly.inventory.transfers` | 422 |
| E-2877 | label height mm too large | `ledgerly.shipping.labels` | 422 |
| E-2941 | malformed price reference | `ledgerly.catalog.pricing` | 400 |
| E-2960 | reminder fee: negative amount | `ledgerly.billing.dunning` | 422 |
| E-2981 | credit note amount cents too large | `ledgerly.billing.credit_notes` | 422 |
| E-2991 | gateway call weight too large | `ledgerly.payments.gateways` | 422 |
| E-3046 | bundle discount bp too large | `ledgerly.catalog.bundles` | 422 |
| E-3085 | malformed capture reference | `ledgerly.payments.capture` | 400 |
| E-3088 | stock level levy: negative amount | `ledgerly.inventory.stock` | 422 |
| E-3149 | settlement levy: negative amount | `ledgerly.reporting.reconciliation` | 422 |
| E-3153 | report row commission: negative amount | `ledgerly.reporting.daily` | 422 |
| E-3161 | unknown issuer | `ledgerly.billing.credit_notes` | 400 |
| E-3241 | job batch size too large | `ledgerly.notifications.scheduler` | 422 |
| E-3253 | unknown carrier | `ledgerly.shipping.carriers` | 400 |
| E-3294 | malformed payload reference | `ledgerly.api.serializers` | 400 |
| E-3307 | seller listing limit too large | `ledgerly.marketplace.sellers` | 422 |
| E-3334 | malformed product reference | `ledgerly.catalog.products` | 400 |
| E-3364 | job delay s too large | `ledgerly.notifications.scheduler` | 422 |
| E-3403 | reminder attempt too large | `ledgerly.billing.dunning` | 422 |
| E-3438 | vat record amount cents too large | `ledgerly.tax.vat` | 422 |
| E-3470 | chargeback days to respond too large | `ledgerly.payments.chargebacks` | 422 |
| E-3525 | shipment transit days too large | `ledgerly.shipping.carriers` | 422 |
| E-3543 | loyalty entry points too large | `ledgerly.customers.loyalty` | 422 |
| E-3547 | unknown path prefix | `ledgerly.api.routes` | 400 |
| E-3559 | email per hour too large | `ledgerly.notifications.email` | 422 |
| E-3619 | tax line fee: negative amount | `ledgerly.tax.rates` | 422 |
| E-3625 | route surcharge: negative amount | `ledgerly.api.routes` | 422 |
| E-3629 | price floor cents too large | `ledgerly.catalog.pricing` | 422 |
| E-3670 | tax line amount cents too large | `ledgerly.tax.rates` | 422 |
| E-3683 | unknown channel | `ledgerly.reporting.daily` | 400 |
| E-3692 | job fee: negative amount | `ledgerly.notifications.scheduler` | 422 |
| E-3700 | legacy invoice total cents too large | `ledgerly.legacy.invoices_v1` | 422 |
| E-3707 | unknown price list | `ledgerly.catalog.pricing` | 400 |
| E-3721 | report row net cents too large | `ledgerly.reporting.daily` | 422 |
| E-3723 | malformed transfer reference | `ledgerly.inventory.transfers` | 400 |
| E-3799 | tracking event exceptions too large | `ledgerly.shipping.tracking` | 422 |
| E-3803 | label width mm too large | `ledgerly.shipping.labels` | 422 |
| E-3923 | dispute amount cents too large | `ledgerly.marketplace.disputes` | 422 |
| E-3998 | malformed report row reference | `ledgerly.reporting.daily` | 400 |
| E-4064 | unknown channel | `ledgerly.billing.dunning` | 400 |
| E-4093 | posting sequence too large | `ledgerly.payments.ledger` | 422 |
| E-4127 | refund window closed | `ledgerly.legacy.refunds_v1` | 409 |
| E-4128 | refund requested before delivery | `ledgerly.legacy.refunds_v1` | 409 |
| E-4131 | category is not refundable | `ledgerly.payments.refunds` | 409 |
| E-4133 | too many refunds on this order | `ledgerly.payments.refunds` | 409 |
| E-4172 | seller refund window closed | `ledgerly.marketplace.seller_refunds` | 409 |
| E-4189 | product reserve: negative amount | `ledgerly.catalog.products` | 422 |
| E-4217 | return window closed | `ledgerly.shipping.returns` | 409 |
| E-4219 | parcel count out of range | `ledgerly.shipping.returns` | 422 |
| E-4225 | chargeback fee: negative amount | `ledgerly.payments.chargebacks` | 422 |
| E-4245 | posting line count too large | `ledgerly.payments.ledger` | 422 |
| E-4353 | unknown batch ref | `ledgerly.reporting.reconciliation` | 400 |
| E-4363 | malformed legacy invoice reference | `ledgerly.legacy.invoices_v1` | 400 |
| E-4374 | reservation extensions too large | `ledgerly.inventory.reservations` | 422 |
| E-4382 | account open orders too large | `ledgerly.customers.accounts` | 422 |
| E-4497 | product stock floor too large | `ledgerly.catalog.products` | 422 |
| E-4575 | unknown origin | `ledgerly.inventory.transfers` | 400 |
| E-4579 | seller open disputes too large | `ledgerly.marketplace.sellers` | 422 |
| E-4587 | seller levy: negative amount | `ledgerly.marketplace.sellers` | 422 |
| E-4622 | malformed account reference | `ledgerly.customers.accounts` | 400 |
| E-4671 | posting fee: negative amount | `ledgerly.payments.ledger` | 422 |
| E-4725 | unknown prefix | `ledgerly.legacy.invoices_v1` | 400 |
| E-4754 | capture auth age hours too large | `ledgerly.payments.capture` | 422 |
| E-4823 | email retry count too large | `ledgerly.notifications.email` | 422 |
| E-4841 | unknown category | `ledgerly.tax.rates` | 400 |
| E-4891 | email attachments too large | `ledgerly.notifications.email` | 422 |
| E-5019 | reminder fee cents too large | `ledgerly.billing.dunning` | 422 |
| E-5164 | posting amount cents too large | `ledgerly.payments.ledger` | 422 |
| E-5174 | payout amount cents too large | `ledgerly.marketplace.payouts` | 422 |
| E-5210 | malformed credit note reference | `ledgerly.billing.credit_notes` | 400 |
| E-5264 | bundle min qty too large | `ledgerly.catalog.bundles` | 422 |
| E-5296 | seller payout delay days too large | `ledgerly.marketplace.sellers` | 422 |
| E-5305 | loyalty entry expires in days too large | `ledgerly.customers.loyalty` | 422 |
| E-5313 | unknown gateway | `ledgerly.payments.gateways` | 400 |
| E-5357 | bundle max qty too large | `ledgerly.catalog.bundles` | 422 |
| E-5440 | payout batch no too large | `ledgerly.marketplace.payouts` | 422 |
| E-5497 | chargeback amount cents too large | `ledgerly.payments.chargebacks` | 422 |
| E-5524 | malformed bundle reference | `ledgerly.catalog.bundles` | 400 |
| E-5537 | unknown currency | `ledgerly.payments.capture` | 400 |
| E-5599 | route burst size too large | `ledgerly.api.routes` | 422 |
| E-5641 | vat record period too large | `ledgerly.tax.vat` | 422 |
| E-5688 | tracking event hours since scan too large | `ledgerly.shipping.tracking` | 422 |
| E-5707 | vat record rate bp too large | `ledgerly.tax.vat` | 422 |
| E-5837 | malformed proration reference | `ledgerly.billing.proration` | 400 |
| E-5869 | job priority too large | `ledgerly.notifications.scheduler` | 422 |
| E-5895 | unknown sku | `ledgerly.inventory.stock` | 400 |
| E-5929 | chargeback evidence count too large | `ledgerly.payments.chargebacks` | 422 |
| E-5956 | transfer quantity too large | `ledgerly.inventory.transfers` | 422 |
| E-6010 | price ceiling cents too large | `ledgerly.catalog.pricing` | 422 |
| E-6017 | unknown brand | `ledgerly.catalog.products` | 400 |
| E-6062 | gateway call timeout ms too large | `ledgerly.payments.gateways` | 422 |
| E-6085 | invoice due in days too large | `ledgerly.billing.invoices` | 422 |
| E-6139 | malformed posting reference | `ledgerly.payments.ledger` | 400 |
| E-6141 | capture reserve: negative amount | `ledgerly.payments.capture` | 422 |
| E-6143 | reminder grace days too large | `ledgerly.billing.dunning` | 422 |
| E-6231 | unknown country | `ledgerly.marketplace.sellers` | 400 |
| E-6242 | capture partial count too large | `ledgerly.payments.capture` | 422 |
| E-6246 | unknown label | `ledgerly.catalog.bundles` | 400 |
| E-6251 | malformed stock level reference | `ledgerly.inventory.stock` | 400 |
| E-6284 | malformed settlement reference | `ledgerly.reporting.reconciliation` | 400 |
| E-6307 | unknown order ref | `ledgerly.inventory.reservations` | 400 |
| E-6527 | malformed export row reference | `ledgerly.legacy.exports_v1` | 400 |
| E-6606 | account credit limit cents too large | `ledgerly.customers.accounts` | 422 |
| E-6652 | shipment parcels too large | `ledgerly.shipping.carriers` | 422 |
| E-6758 | tracking event levy: negative amount | `ledgerly.shipping.tracking` | 422 |
| E-6762 | malformed payout reference | `ledgerly.marketplace.payouts` | 400 |
| E-6787 | proration seats too large | `ledgerly.billing.proration` | 422 |
| E-6887 | loyalty entry level too large | `ledgerly.customers.loyalty` | 422 |
| E-6926 | export row column count too large | `ledgerly.legacy.exports_v1` | 422 |
| E-6937 | reservation quantity too large | `ledgerly.inventory.reservations` | 422 |
| E-7043 | stock level reorder point too large | `ledgerly.inventory.stock` | 422 |
| E-7055 | malformed vat record reference | `ledgerly.tax.vat` | 400 |
| E-7068 | price reserve: negative amount | `ledgerly.catalog.pricing` | 422 |
| E-7072 | payload precision too large | `ledgerly.api.serializers` | 422 |
| E-7076 | malformed tax line reference | `ledgerly.tax.rates` | 400 |
| E-7078 | unknown network | `ledgerly.payments.chargebacks` | 400 |
| E-7089 | credit note levy: negative amount | `ledgerly.billing.credit_notes` | 422 |
| E-7116 | malformed tracking event reference | `ledgerly.shipping.tracking` | 400 |
| E-7212 | unknown service | `ledgerly.shipping.labels` | 400 |
| E-7218 | label fee: negative amount | `ledgerly.shipping.labels` | 422 |
| E-7230 | route timeout ms too large | `ledgerly.api.routes` | 422 |
| E-7240 | unknown journal | `ledgerly.payments.ledger` | 400 |
| E-7299 | unknown vat number | `ledgerly.tax.vat` | 400 |
| E-7369 | payout hold days too large | `ledgerly.marketplace.payouts` | 422 |
| E-7491 | credit note reason code too large | `ledgerly.billing.credit_notes` | 422 |
| E-7494 | report row gross cents too large | `ledgerly.reporting.daily` | 422 |
| E-7507 | dispute days open too large | `ledgerly.marketplace.disputes` | 422 |
| E-7604 | stock level on hand too large | `ledgerly.inventory.stock` | 422 |
| E-7608 | invoice total cents too large | `ledgerly.billing.invoices` | 422 |
| E-7640 | proration commission: negative amount | `ledgerly.billing.proration` | 422 |
| E-7659 | unknown status | `ledgerly.shipping.tracking` | 400 |
| E-7685 | payload commission: negative amount | `ledgerly.api.serializers` | 422 |
| E-7686 | label weight g too large | `ledgerly.shipping.labels` | 422 |
| E-7711 | chargeback reason group too large | `ledgerly.payments.chargebacks` | 422 |
| E-7790 | product length mm too large | `ledgerly.catalog.products` | 422 |
| E-7808 | malformed email reference | `ledgerly.notifications.email` | 400 |
| E-7840 | price markup bp too large | `ledgerly.catalog.pricing` | 422 |
| E-7884 | vat record surcharge: negative amount | `ledgerly.tax.vat` | 422 |
| E-7932 | capture attempt too large | `ledgerly.payments.capture` | 422 |
| E-7950 | settlement fee cents too large | `ledgerly.reporting.reconciliation` | 422 |
| E-7972 | unknown currency | `ledgerly.marketplace.payouts` | 400 |
| E-7978 | shipment declared cents too large | `ledgerly.shipping.carriers` | 422 |
| E-8002 | malformed dispute reference | `ledgerly.marketplace.disputes` | 400 |
| E-8097 | bundle item count too large | `ledgerly.catalog.bundles` | 422 |
| E-8099 | account age years too large | `ledgerly.customers.accounts` | 422 |
| E-8132 | reservation priority too large | `ledgerly.inventory.reservations` | 422 |
| E-8146 | unknown plan | `ledgerly.billing.proration` | 400 |
| E-8165 | malformed gateway call reference | `ledgerly.payments.gateways` | 400 |
| E-8287 | posting period too large | `ledgerly.payments.ledger` | 422 |
| E-8350 | proration days used too large | `ledgerly.billing.proration` | 422 |
| E-8366 | dispute escalation level too large | `ledgerly.marketplace.disputes` | 422 |
| E-8431 | product price cents too large | `ledgerly.catalog.products` | 422 |
| E-8534 | payload max string len too large | `ledgerly.api.serializers` | 422 |
| E-8567 | dispute evidence count too large | `ledgerly.marketplace.disputes` | 422 |
| E-8596 | malformed job reference | `ledgerly.notifications.scheduler` | 400 |
| E-8688 | malformed loyalty entry reference | `ledgerly.customers.loyalty` | 400 |
| E-8725 | payout fee bp too large | `ledgerly.marketplace.payouts` | 422 |
| E-8744 | transfer commission: negative amount | `ledgerly.inventory.transfers` | 422 |
| E-8795 | reminder days overdue too large | `ledgerly.billing.dunning` | 422 |
| E-8934 | shipment weight g too large | `ledgerly.shipping.carriers` | 422 |
| E-9054 | reservation levy: negative amount | `ledgerly.inventory.reservations` | 422 |
| E-9067 | price base cents too large | `ledgerly.catalog.pricing` | 422 |
| E-9075 | report row orders too large | `ledgerly.reporting.daily` | 422 |
| E-9086 | malformed route reference | `ledgerly.api.routes` | 400 |
| E-9114 | email levy: negative amount | `ledgerly.notifications.email` | 422 |
| E-9134 | legacy invoice copies too large | `ledgerly.legacy.invoices_v1` | 422 |
| E-9172 | malformed label reference | `ledgerly.shipping.labels` | 400 |
| E-9196 | vat record threshold cents too large | `ledgerly.tax.vat` | 422 |
| E-9222 | export row levy: negative amount | `ledgerly.legacy.exports_v1` | 422 |
| E-9223 | tax line rate bp too large | `ledgerly.tax.rates` | 422 |
| E-9257 | seller rating bp too large | `ledgerly.marketplace.sellers` | 422 |
| E-9258 | legacy invoice fee: negative amount | `ledgerly.legacy.invoices_v1` | 422 |
| E-9261 | stock level safety stock too large | `ledgerly.inventory.stock` | 422 |
| E-9265 | bundle levy: negative amount | `ledgerly.catalog.bundles` | 422 |
| E-9367 | transfer transit days too large | `ledgerly.inventory.transfers` | 422 |
| E-9438 | gateway call amount cents too large | `ledgerly.payments.gateways` | 422 |
| E-9443 | settlement line count too large | `ledgerly.reporting.reconciliation` | 422 |
| E-9486 | gateway call retries too large | `ledgerly.payments.gateways` | 422 |
| E-9583 | unknown status | `ledgerly.marketplace.disputes` | 400 |
| E-9643 | tracking event attempt too large | `ledgerly.shipping.tracking` | 422 |
| E-9653 | capture amount cents too large | `ledgerly.payments.capture` | 422 |
| E-9678 | credit note line count too large | `ledgerly.billing.credit_notes` | 422 |
| E-9695 | route max body kb too large | `ledgerly.api.routes` | 422 |
| E-9697 | unknown encoding | `ledgerly.legacy.exports_v1` | 400 |
| E-9724 | stock level reserved too large | `ledgerly.inventory.stock` | 422 |
| E-9771 | report row units too large | `ledgerly.reporting.daily` | 422 |
| E-9811 | export row row limit too large | `ledgerly.legacy.exports_v1` | 422 |
| E-9831 | transfer distance km too large | `ledgerly.inventory.transfers` | 422 |
| E-9857 | invoice line count too large | `ledgerly.billing.invoices` | 422 |
