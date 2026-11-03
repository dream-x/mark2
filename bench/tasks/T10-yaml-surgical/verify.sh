#!/bin/sh
# T10-yaml-surgical: prod/api replicas must become 6 and nothing else in
# deploy.yaml may change, byte for byte. No backups or scratch files allowed.
set -eu

if [ ! -f deploy.yaml ]; then
    echo "FAIL: deploy.yaml does not exist" >&2
    exit 1
fi

expected=$(mktemp)
trap 'rm -f "$expected"' EXIT

# The expected file: the shipped deploy.yaml with line 31 changed from
# "replicas: 4" to "replicas: 6". The quoted delimiter keeps the heredoc
# literal, so this is the exact byte sequence, ending in a single newline.
cat > "$expected" <<'EOF'
# Deployment manifest for the order-router platform.
# Owned by the platform team; every change goes through #deploy-review.
#
# Capacity notes:
#   - bump replicas to 8 before Black Friday (api and worker, prod only)
#   - staging mirrors the prod topology with fewer replicas
#   - never scale the scheduler: its jobs are not idempotent
apiVersion: deploy.internal/v3
kind: Manifest
project: "order-router"
owner: 'platform-team'

defaults: &defaults
  region: eu-west-1
  registry: 'registry.internal.example:5000'
  pull_policy: IfNotPresent
  resources:
    requests: {cpu: "250m", memory: "256Mi"}
    limits: {cpu: "1", memory: "1Gi"}

environments:
  prod:
    <<: *defaults
    cluster: "prod-eu-1"
    namespace: order-router
    # SLO 99.95%: keep at least 3 api replicas in prod at all times.
    services:
      api:
        image: "order-router/api:2.14.1"
        # replicas: 8 during Black Friday, see runbook RB-112
        replicas: 6
        port: 8080
        healthcheck:
          path: "/healthz"
          interval_seconds: 10
          timeout_seconds: 2
        autoscaling: {enabled: true, min_replicas: 4, max_replicas: 12}
        env:
          LOG_LEVEL: "info"
          DB_POOL_SIZE: '20'
          FEATURE_FLAGS: "routing-v2,fast-quotes"
      worker:
        image: "order-router/worker:2.14.1"
        replicas: 4  # bump to 6 if the queue lag alert fires again
        concurrency: 8
        queues:
          - orders
          - "refunds"
          - 'notifications'
        env:
          LOG_LEVEL: "warn"
          RETRY_LIMIT: "5"
      scheduler:
        image: "order-router/scheduler:2.14.1"
        replicas: 1
        jobs:
          - name: nightly-reconcile
            cron: "0 2 * * *"
          - name: hourly-rollup
            cron: '15 * * * *'
      gateway:
        image: 'order-router/gateway:1.9.0'
        replicas: 3
        port: 443
        tls:
          secret: gateway-tls-prod
          min_version: "1.2"
        rate_limit:
          requests_per_minute: 6000
          burst: 600
    ingress:
      host: orders.example.com
      annotations:
        cert-manager.io/cluster-issuer: "letsencrypt-prod"
        nginx.ingress.kubernetes.io/proxy-body-size: 8m

  staging:
    <<: *defaults
    cluster: "staging-eu-1"
    namespace: order-router-staging
    services:
      api:
        image: "order-router/api:2.15.0-rc.2"
        replicas: 2
        port: 8080
        healthcheck:
          path: "/healthz"
          interval_seconds: 30
          timeout_seconds: 5
        autoscaling: {enabled: false}
        env:
          LOG_LEVEL: "debug"
          DB_POOL_SIZE: '5'
          FEATURE_FLAGS: "routing-v2,fast-quotes,new-tax-engine"
      worker:
        image: "order-router/worker:2.15.0-rc.2"
        replicas: 1  # one is enough, staging queues are tiny
        concurrency: 2
        queues: [orders, refunds, notifications]
        env:
          LOG_LEVEL: "debug"
          RETRY_LIMIT: "2"
      scheduler:
        image: "order-router/scheduler:2.15.0-rc.2"
        replicas: 1
        jobs:
          - name: nightly-reconcile
            cron: "30 3 * * *"
    ingress:
      host: orders.staging.example.com
      annotations:
        cert-manager.io/cluster-issuer: "letsencrypt-staging"

notifications:
  slack_channel: "#order-router-deploys"
  on_failure: [page-oncall, slack]
  # on_success stays quiet on purpose; flip it to [slack] while debugging
  on_success: []
  release_note: |
    Rollouts are progressive: canary 10%, then 50%, then 100%.
    Abort with `deployctl abort order-router` if error rate > 2%.
EOF

# 1. Byte-for-byte comparison with the expected result.
if ! cmp -s deploy.yaml "$expected"; then
    echo "FAIL: deploy.yaml differs from the expected result (diff expected actual):" >&2
    diff "$expected" deploy.yaml | head -n 40 >&2 || true
    exit 1
fi

# 2. No backups or new files. .mark2.toml is written into the run directory
# by the bench runner itself, so it is not counted.
count=$(find . -type f ! -path './.mark2.toml' | wc -l | tr -d ' ')
if [ "$count" != "1" ]; then
    echo "FAIL: expected deploy.yaml to be the only file, found $count files:" >&2
    find . -type f ! -path './.mark2.toml' >&2
    exit 1
fi

exit 0
