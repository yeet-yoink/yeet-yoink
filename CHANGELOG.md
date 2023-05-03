# Changelog

All notable changes to this project will be documented in this file.
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- Health check endpoints are available at `/startupz`, `/readyz`,
  `/livez`, as well as `/health` and `/healthz`.
- Prometheus/OpenMetrics metrics is available at `/metrics`.
- Shutdown from SIGINT (e.g. CTRL-C), SIGTERM and similar is now possible.

### Internal

- 🎉 Initial release.
