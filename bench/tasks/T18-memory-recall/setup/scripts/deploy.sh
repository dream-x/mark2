#!/bin/sh
# Deploy helper used by CI: scripts/deploy.sh <environment>
set -eu

env=${1:?usage: deploy.sh <environment>}
case "$env" in
    production)
        port=$(sed -n 's/^port *= *//p' deploy.ini)
        ;;
    staging)
        # CI passes the staging port in; it is not stored in the repository.
        port=${STAGING_PORT:?STAGING_PORT is not set}
        ;;
    *)
        echo "unknown environment: $env" >&2
        exit 2
        ;;
esac

echo "deploying $env on port $port"
