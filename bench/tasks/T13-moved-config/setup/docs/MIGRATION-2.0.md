# Migrating to the 2.0 layout

2.0 splits the monolithic `app/` tree into independent services under
`services/`. Each service can now be deployed and configured on its own.

## Configuration

- Each service owns its configuration and ships it inside its own directory
  tree. Settings moved together with the code that reads them.
- The shared `app/config/` directory is no longer read by anything. It is kept
  only so that 1.x deployments can be compared with 2.0.
- Many values were re-tuned during the move. Do not copy numbers from the 1.x
  files into a 2.0 deployment.
- The cache settings moved with the service that owns the cache.

## Tests

Test fixtures are deliberately tiny and do not mirror production values.
