# Changelog

All notable changes to this project will be documented in this file.
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- The `/yeet` endpoint now supports the optional `?file_name=...` parameter for specifying
  the original file name as metadata to be returned with `/yoink`.

## [0.0.1] - 2023-06-25

### Added

- The service can be used to store and retrieve files locally via the `/yeet` and `/yoink/:id` endpoints.
- Health check endpoints are available at `/startupz`, `/readyz`,
  `/livez`, as well as `/health` and `/healthz`.
- Prometheus/OpenMetrics metrics is available at `/metrics`.
- Shutdown from SIGINT (e.g. CTRL-C), SIGTERM and similar is now possible.
- The service now has a `/stop` endpoint to gracefully shut it down, freeing all open resources.
- The `transfer_size_total` and `transfer_total` metrics now track the number of bytes sent through the service.

### Internal

- 🎉 Initial release.

[0.0.1]: https://github.com/sunsided/yeet-yoink/releases/tag/0.0.1
